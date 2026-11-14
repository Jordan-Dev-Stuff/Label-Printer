//! Bluetooth LE printing.
//!
//! This is the path phone apps such as Label Expert use: Bluetooth Low Energy
//! needs no pairing and therefore no PIN. (Windows' "Add device" pairs the
//! printer over Bluetooth *Classic*, which is what asks for a PIN.)
//!
//! The UI thread never blocks: it sends [`BleCommand`]s and polls
//! [`BleEvent`]s. The worker owns a single-threaded Tokio runtime, the
//! adapter, and the connected peripheral.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};

/// Bytes per GATT write. 180 fits the MTU these printers negotiate.
const CHUNK: usize = 180;
/// Pause between unacknowledged writes, so the printer's buffer keeps up.
const PACING: Duration = Duration::from_millis(20);
/// Pause between unacknowledged writes in fast mode.
const FAST_PACING: Duration = Duration::from_millis(4);
/// In fast mode, every this many chunks is acknowledged (flow control).
const SYNC_EVERY: usize = 16;
/// Longest scan.
const SCAN_TIME: Duration = Duration::from_secs(6);
/// Longest scan when looking for a remembered printer; it stops as soon as
/// the printer is seen. The first scan after start-up can be slow on Windows.
const RECONNECT_SCAN_TIME: Duration = Duration::from_secs(15);

/// Characteristics known to carry print data, most specific first.
///
/// * `ff02` — the 0xFF00 service used by Phomemo and many TSPL/ESC-POS printers.
/// * `49535343-8841-…` — Microchip/ISSC "transparent UART", common in BEEPRT units.
/// * `ae01` — the 0xAE30 service of many generic label printers.
/// * `bef8d6c9-…` — the Gprinter / Xprinter data channel.
const KNOWN_WRITE_UUIDS: &[&str] = &[
    "0000ff02-0000-1000-8000-00805f9b34fb",
    "49535343-8841-43f4-a8d4-ecbe34729bb3",
    "0000ae01-0000-1000-8000-00805f9b34fb",
    "bef8d6c9-9c21-4c9e-b632-bd58c1009f9f",
    "0000ff01-0000-1000-8000-00805f9b34fb",
];

/// Standard GATT services whose writable characteristics are never print
/// channels (Generic Access holds the device name, which is often writable).
const STANDARD_SERVICES: &[&str] = &[
    "00001800-0000-1000-8000-00805f9b34fb",
    "00001801-0000-1000-8000-00805f9b34fb",
    "0000180a-0000-1000-8000-00805f9b34fb",
    "0000180f-0000-1000-8000-00805f9b34fb",
];

/// One discovered BLE device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Opaque platform id.
    pub id: String,
    /// Advertised local name, or a placeholder.
    pub name: String,
    /// Signal strength, when reported.
    pub rssi: Option<i16>,
}

/// Work for the worker thread.
#[derive(Debug)]
pub enum BleCommand {
    /// Scan and report every advertiser.
    Scan,
    /// Connect to a device id from a scan.
    Connect(String),
    /// Scan for a remembered device (by id, else by name) and connect.
    Reconnect {
        /// Platform id from last time.
        id: String,
        /// Advertised name from last time.
        name: String,
    },
    /// Write a job; the shared flag cancels it between chunks.
    Print {
        /// Encoded job.
        job: Vec<u8>,
        /// Use the faster streamed write mode.
        fast: bool,
    },
    /// Drop the connection.
    Disconnect,
}

/// Progress reported back to the UI.
#[derive(Debug)]
pub enum BleEvent {
    /// A scan has started.
    Scanning,
    /// Scan finished with these devices.
    Devices(Vec<DeviceInfo>),
    /// Connected and found a print channel.
    Connected {
        /// Platform id, to remember.
        id: String,
        /// Advertised name.
        name: String,
        /// UUID of the characteristic chosen for writes.
        channel: String,
    },
    /// `sent` of `total` bytes written.
    Progress {
        /// Bytes sent so far.
        sent: usize,
        /// Job size.
        total: usize,
    },
    /// A job finished.
    Printed(usize),
    /// A job was cancelled part-way.
    Cancelled,
    /// A remembered printer was not found this time; the UI retries.
    NotFound {
        /// Printer looked for.
        name: String,
        /// What happened, for the activity log.
        detail: String,
    },
    /// The session was dropped.
    Disconnected,
    /// Something failed; the string is user-facing.
    Error(String),
}

/// Handle to the worker.
pub struct Ble {
    /// Commands in.
    pub commands: Sender<BleCommand>,
    /// Events out.
    pub events: Receiver<BleEvent>,
    /// Set to stop the current print.
    pub cancel: Arc<AtomicBool>,
}

/// Starts the worker. `wake` is called after every event so the UI repaints.
#[must_use]
pub fn spawn(wake: impl Fn() + Send + 'static) -> Ble {
    let (command_tx, command_rx) = channel();
    let (event_tx, event_rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);

    // The thread ends when the UI drops its command sender.
    std::thread::Builder::new()
        .name("bluetooth".to_string())
        .spawn(move || {
            let send = |event: BleEvent| {
                let _ = event_tx.send(event);
                wake();
            };
            worker(&command_rx, &send, &worker_cancel);
        })
        .expect("spawn bluetooth thread");

    Ble {
        commands: command_tx,
        events: event_rx,
        cancel,
    }
}

/// A connected device and its print channel.
struct Session {
    peripheral: Peripheral,
    characteristic: Characteristic,
    name: String,
}

fn worker(commands: &Receiver<BleCommand>, send: &dyn Fn(BleEvent), cancel: &AtomicBool) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            send(BleEvent::Error(format!("Bluetooth runtime: {error}")));
            return;
        }
    };

    let mut adapter: Option<Adapter> = None;
    let mut session: Option<Session> = None;

    while let Ok(command) = commands.recv() {
        if adapter.is_none() {
            match runtime.block_on(first_adapter()) {
                Ok(found) => adapter = Some(found),
                Err(error) => {
                    send(BleEvent::Error(error));
                    continue;
                }
            }
        }
        let Some(adapter) = adapter.as_ref() else {
            continue;
        };

        match command {
            BleCommand::Scan => {
                send(BleEvent::Scanning);
                match runtime.block_on(scan(adapter, None, SCAN_TIME)) {
                    Ok(devices) => send(BleEvent::Devices(devices)),
                    Err(error) => send(BleEvent::Error(error)),
                }
            }
            BleCommand::Connect(id) => {
                if let Some(old) = session.take() {
                    let _ = runtime.block_on(old.peripheral.disconnect());
                }
                match runtime.block_on(connect(adapter, &id)) {
                    Ok(new) => {
                        send(connected_event(&new));
                        session = Some(new);
                    }
                    Err(error) => send(BleEvent::Error(error)),
                }
            }
            BleCommand::Reconnect { id, name } => {
                if session.is_some() {
                    continue;
                }
                let devices = match runtime.block_on(scan(
                    adapter,
                    Some((&id, &name)),
                    RECONNECT_SCAN_TIME,
                )) {
                    Ok(devices) => devices,
                    Err(error) => {
                        send(BleEvent::NotFound {
                            name,
                            detail: error,
                        });
                        continue;
                    }
                };
                match find_remembered(&devices, &id, &name) {
                    Some(target) => match runtime.block_on(connect(adapter, &target)) {
                        Ok(new) => {
                            send(connected_event(&new));
                            session = Some(new);
                        }
                        Err(error) => send(BleEvent::NotFound {
                            name,
                            detail: error,
                        }),
                    },
                    None => send(BleEvent::NotFound {
                        name,
                        detail: format!(
                            "not seen in a {} s scan ({} other device(s) were)",
                            RECONNECT_SCAN_TIME.as_secs(),
                            devices.len()
                        ),
                    }),
                }
            }
            BleCommand::Print { job, fast } => {
                cancel.store(false, Ordering::SeqCst);
                let Some(current) = session.as_ref() else {
                    send(BleEvent::Error(
                        "connect to a Bluetooth printer first".to_string(),
                    ));
                    continue;
                };
                // A printer that went to sleep drops the link; reconnect once.
                let alive = runtime
                    .block_on(current.peripheral.is_connected())
                    .unwrap_or(false);
                if !alive && runtime.block_on(current.peripheral.connect()).is_err() {
                    send(BleEvent::Error(format!(
                        "lost the connection to {}; switch it on and connect again",
                        current.name
                    )));
                    session = None;
                    send(BleEvent::Disconnected);
                    continue;
                }
                match runtime.block_on(write_job(current, &job, fast, cancel, send)) {
                    Ok(true) => send(BleEvent::Printed(job.len())),
                    Ok(false) => send(BleEvent::Cancelled),
                    Err(error) => send(BleEvent::Error(error)),
                }
            }
            BleCommand::Disconnect => {
                if let Some(old) = session.take() {
                    let _ = runtime.block_on(old.peripheral.disconnect());
                }
                send(BleEvent::Disconnected);
            }
        }
    }
}

fn connected_event(session: &Session) -> BleEvent {
    BleEvent::Connected {
        id: format!("{:?}", session.peripheral.id()),
        name: session.name.clone(),
        channel: session.characteristic.uuid.to_string(),
    }
}

async fn first_adapter() -> Result<Adapter, String> {
    let manager = Manager::new()
        .await
        .map_err(|error| format!("Bluetooth unavailable: {error}"))?;
    manager
        .adapters()
        .await
        .map_err(|error| format!("Bluetooth unavailable: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "no Bluetooth adapter found (is Bluetooth switched on?)".to_string())
}

/// The id to connect to when `devices` contains the remembered printer:
/// the same platform id, or else the same advertised name (ignoring case and
/// surrounding spaces), since some stacks hand out a new id per session.
#[must_use]
pub fn find_remembered(devices: &[DeviceInfo], id: &str, name: &str) -> Option<String> {
    let name = name.trim();
    devices
        .iter()
        .find(|d| d.id == id)
        .or_else(|| {
            devices
                .iter()
                .find(|d| !name.is_empty() && d.name.trim().eq_ignore_ascii_case(name))
        })
        .map(|d| d.id.clone())
}

/// Scans for up to `limit`, stopping early once `wanted` (id, name) shows up.
async fn scan(
    adapter: &Adapter,
    wanted: Option<(&str, &str)>,
    limit: Duration,
) -> Result<Vec<DeviceInfo>, String> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|error| format!("start scan: {error}"))?;

    let started = Instant::now();
    let mut found = Vec::new();
    while started.elapsed() < limit {
        tokio::time::sleep(Duration::from_millis(500)).await;
        found = list(adapter).await?;
        if let Some((id, name)) = wanted {
            if find_remembered(&found, id, name).is_some() {
                break;
            }
        }
    }

    let _ = adapter.stop_scan().await;
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

async fn list(adapter: &Adapter) -> Result<Vec<DeviceInfo>, String> {
    let mut devices = Vec::new();
    for peripheral in adapter.peripherals().await.map_err(|e| e.to_string())? {
        let properties = peripheral.properties().await.ok().flatten();
        let name = properties
            .as_ref()
            .and_then(|p| p.local_name.clone())
            .unwrap_or_else(|| "(unnamed)".to_string());
        devices.push(DeviceInfo {
            id: format!("{:?}", peripheral.id()),
            name,
            rssi: properties.and_then(|p| p.rssi),
        });
    }
    Ok(devices)
}

async fn connect(adapter: &Adapter, id: &str) -> Result<Session, String> {
    let peripheral = adapter
        .peripherals()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|candidate| format!("{:?}", candidate.id()) == id)
        .ok_or_else(|| "device is no longer in range; scan again".to_string())?;

    let name = peripheral
        .properties()
        .await
        .ok()
        .flatten()
        .and_then(|p| p.local_name)
        .unwrap_or_else(|| "Bluetooth printer".to_string());

    if !peripheral.is_connected().await.unwrap_or(false) {
        peripheral
            .connect()
            .await
            .map_err(|error| format!("connect to {name}: {error}"))?;
    }
    peripheral
        .discover_services()
        .await
        .map_err(|error| format!("read services of {name}: {error}"))?;

    let characteristics: Vec<Characteristic> = peripheral.characteristics().into_iter().collect();
    let infos: Vec<ChannelInfo> = characteristics
        .iter()
        .map(|c| ChannelInfo {
            uuid: c.uuid.to_string(),
            service: c.service_uuid.to_string(),
            write: c.properties.contains(CharPropFlags::WRITE),
            write_without_response: c.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE),
        })
        .collect();

    let Some(index) = choose_channel(&infos) else {
        let _ = peripheral.disconnect().await;
        return Err(format!(
            "{name} has no printer data channel; it may not be a printer, or it may only \
             support Bluetooth Classic"
        ));
    };

    Ok(Session {
        characteristic: characteristics[index].clone(),
        peripheral,
        name,
    })
}

/// What [`choose_channel`] needs to know about a characteristic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    /// Characteristic UUID, any case.
    pub uuid: String,
    /// UUID of the service it belongs to.
    pub service: String,
    /// Supports acknowledged writes.
    pub write: bool,
    /// Supports unacknowledged writes.
    pub write_without_response: bool,
}

/// Picks the characteristic to send print data to.
///
/// Known printer channels win in [`KNOWN_WRITE_UUIDS`] order. Otherwise the
/// first writable characteristic outside the standard GATT services, which
/// rules out the writable device-name field the old code used to pick.
#[must_use]
pub fn choose_channel(channels: &[ChannelInfo]) -> Option<usize> {
    let writable = |c: &ChannelInfo| c.write || c.write_without_response;

    for known in KNOWN_WRITE_UUIDS {
        if let Some(index) = channels
            .iter()
            .position(|c| writable(c) && c.uuid.eq_ignore_ascii_case(known))
        {
            return Some(index);
        }
    }

    channels.iter().position(|c| {
        writable(c)
            && !STANDARD_SERVICES
                .iter()
                .any(|service| c.service.eq_ignore_ascii_case(service))
    })
}

/// How chunks are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Every chunk acknowledged: the printer paces us. Slowest, safest.
    Acknowledged,
    /// Unacknowledged chunks, `pacing` apart, with an acknowledged chunk every
    /// `sync_every` chunks (when the channel supports it) so a slow printer
    /// can still hold us back.
    Streamed {
        /// Pause after each unacknowledged chunk.
        pacing: Duration,
        /// Acknowledged write interval, in chunks; `0` = never.
        sync_every: usize,
    },
}

/// Picks the write mode for a channel's capabilities and the user's choice.
#[must_use]
pub fn write_mode(acknowledged: bool, unacknowledged: bool, fast: bool) -> WriteMode {
    match (acknowledged, unacknowledged, fast) {
        (true, true, true) => WriteMode::Streamed {
            pacing: FAST_PACING,
            sync_every: SYNC_EVERY,
        },
        (true, _, _) => WriteMode::Acknowledged,
        (false, _, true) => WriteMode::Streamed {
            pacing: FAST_PACING,
            sync_every: 0,
        },
        (false, _, false) => WriteMode::Streamed {
            pacing: PACING,
            sync_every: 0,
        },
    }
}

/// Writes `job` in chunks. Returns `Ok(false)` when cancelled.
async fn write_job(
    session: &Session,
    job: &[u8],
    fast: bool,
    cancel: &AtomicBool,
    send: &dyn Fn(BleEvent),
) -> Result<bool, String> {
    let properties = session.characteristic.properties;
    let mode = write_mode(
        properties.contains(CharPropFlags::WRITE),
        properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE),
        fast,
    );

    let mut sent = 0;
    let mut last_report = Instant::now();
    send(BleEvent::Progress {
        sent: 0,
        total: job.len(),
    });
    for (index, chunk) in job.chunks(CHUNK).enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let (write_type, pause) = match mode {
            WriteMode::Acknowledged => (WriteType::WithResponse, None),
            WriteMode::Streamed { pacing, sync_every } => {
                if sync_every > 0 && (index + 1) % sync_every == 0 {
                    (WriteType::WithResponse, None)
                } else {
                    (WriteType::WithoutResponse, Some(pacing))
                }
            }
        };
        session
            .peripheral
            .write(&session.characteristic, chunk, write_type)
            .await
            .map_err(|error| format!("Bluetooth write failed after {sent} bytes: {error}"))?;
        sent += chunk.len();
        if let Some(pause) = pause {
            tokio::time::sleep(pause).await;
        }
        if last_report.elapsed() > Duration::from_millis(100) {
            send(BleEvent::Progress {
                sent,
                total: job.len(),
            });
            last_report = Instant::now();
        }
    }
    send(BleEvent::Progress {
        sent,
        total: job.len(),
    });
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(uuid: &str, service: &str, write: bool, wwr: bool) -> ChannelInfo {
        ChannelInfo {
            uuid: uuid.to_string(),
            service: service.to_string(),
            write,
            write_without_response: wwr,
        }
    }

    const GAP: &str = "00001800-0000-1000-8000-00805f9b34fb";
    const DEVICE_NAME: &str = "00002a00-0000-1000-8000-00805f9b34fb";

    #[test]
    fn known_printer_channel_beats_order() {
        let channels = [
            channel(DEVICE_NAME, GAP, true, false),
            channel(
                "0000fff2-0000-1000-8000-00805f9b34fb",
                "0000fff0-0000-1000-8000-00805f9b34fb",
                true,
                false,
            ),
            channel(
                "0000FF02-0000-1000-8000-00805F9B34FB",
                "0000ff00-0000-1000-8000-00805f9b34fb",
                false,
                true,
            ),
        ];
        assert_eq!(choose_channel(&channels), Some(2));
    }

    #[test]
    fn writable_device_name_is_never_chosen() {
        let channels = [channel(DEVICE_NAME, GAP, true, true)];
        assert_eq!(choose_channel(&channels), None);
    }

    #[test]
    fn unknown_vendor_channel_is_the_fallback() {
        let channels = [
            channel(DEVICE_NAME, GAP, true, false),
            channel(
                "0000fff1-0000-1000-8000-00805f9b34fb",
                "0000fff0-0000-1000-8000-00805f9b34fb",
                false,
                false,
            ),
            channel(
                "0000fff2-0000-1000-8000-00805f9b34fb",
                "0000fff0-0000-1000-8000-00805f9b34fb",
                true,
                false,
            ),
        ];
        assert_eq!(choose_channel(&channels), Some(2));
    }

    #[test]
    fn read_only_known_uuid_is_skipped() {
        let channels = [channel(
            KNOWN_WRITE_UUIDS[0],
            "0000ff00-0000-1000-8000-00805f9b34fb",
            false,
            false,
        )];
        assert_eq!(choose_channel(&channels), None);
        assert_eq!(choose_channel(&[]), None);
    }

    #[test]
    fn write_modes_follow_capabilities_and_choice() {
        // Reliable: acknowledged whenever the channel supports it.
        assert_eq!(write_mode(true, true, false), WriteMode::Acknowledged);
        assert_eq!(write_mode(true, false, true), WriteMode::Acknowledged);
        // Fast with both: streamed, with periodic acknowledged syncs.
        assert_eq!(
            write_mode(true, true, true),
            WriteMode::Streamed {
                pacing: FAST_PACING,
                sync_every: SYNC_EVERY
            }
        );
        // Unacknowledged-only channels can never sync.
        assert_eq!(
            write_mode(false, true, false),
            WriteMode::Streamed {
                pacing: PACING,
                sync_every: 0
            }
        );
        assert_eq!(
            write_mode(false, true, true),
            WriteMode::Streamed {
                pacing: FAST_PACING,
                sync_every: 0
            }
        );
    }

    fn device(id: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            id: id.to_string(),
            name: name.to_string(),
            rssi: None,
        }
    }

    #[test]
    fn remembered_printer_is_found_by_id_then_name() {
        let devices = [
            device("PeripheralId(AA:AA:AA:AA:AA:AA)", "Soundcore"),
            device("PeripheralId(DC:0D:30:00:F2:CE)", "BY-482BT_F2CE-LE"),
        ];
        // Same id.
        assert_eq!(
            find_remembered(&devices, "PeripheralId(DC:0D:30:00:F2:CE)", "anything").as_deref(),
            Some("PeripheralId(DC:0D:30:00:F2:CE)")
        );
        // New id, same name (case and spaces ignored).
        assert_eq!(
            find_remembered(&devices, "PeripheralId(old)", " by-482bt_f2ce-le ").as_deref(),
            Some("PeripheralId(DC:0D:30:00:F2:CE)")
        );
        // Not there, and an empty name never matches unnamed devices.
        assert_eq!(find_remembered(&devices, "nope", "BY-482BT_0000-LE"), None);
        assert_eq!(find_remembered(&[device("x", "")], "nope", ""), None);
        assert_eq!(find_remembered(&[], "x", "y"), None);
    }
}
