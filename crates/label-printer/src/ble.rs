//! Bluetooth LE printing.
//!
//! This is the path phone apps such as Label Expert use: Bluetooth Low Energy
//! needs no pairing and therefore no PIN. (Windows' "Add device" pairs the
//! printer over Bluetooth *Classic*, which is what asks for a PIN.)
//!
//! On Linux, a dual-mode printer is printed to over its Bluetooth Classic
//! serial channel (RFCOMM) instead, when it answers there: bluetoothd 5.88
//! leaves such printers with an empty GATT table, and RFCOMM needs neither
//! GATT nor pairing. The LE side is only a fallback there.
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
/// Extra service reads when the first comes back without a print channel.
const DISCOVERY_RETRIES: u32 = 4;
/// Wait between those reads (about 8 s in total).
const DISCOVERY_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Connection attempts before giving up on a dropped LE connection.
const CONNECT_ATTEMPTS: u32 = 3;
/// Wait between connection attempts.
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(1500);
/// Bytes per RFCOMM write, so progress and cancel stay responsive.
const SERIAL_CHUNK: usize = 4096;
/// Prefix of a remembered id that names a Bluetooth Classic address.
const SERIAL_ID_PREFIX: &str = "rfcomm:";
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
    link: Link,
    name: String,
}

/// How print data reaches the printer.
enum Link {
    /// A GATT characteristic on the LE connection.
    Gatt {
        peripheral: Peripheral,
        characteristic: Characteristic,
    },
    /// The Bluetooth Classic serial channel, opened per job. Holds the
    /// printer's Classic address.
    Serial(String),
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
                    runtime.block_on(old.disconnect());
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
                // A Classic printer is reached by address; no scan needed.
                if let Some(address) = id.strip_prefix(SERIAL_ID_PREFIX) {
                    match open_serial(&name, address) {
                        Ok(new) => {
                            send(connected_event(&new));
                            session = Some(new);
                        }
                        Err(error) => send(BleEvent::NotFound {
                            name,
                            detail: error,
                        }),
                    }
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
                // `None`: the link is gone and could not be reopened.
                let result = match &current.link {
                    Link::Serial(address) => Some(write_serial(address, &job, cancel, send)),
                    Link::Gatt {
                        peripheral,
                        characteristic,
                    } => {
                        // A printer that went to sleep drops the link; reconnect once.
                        let alive = runtime.block_on(peripheral.is_connected()).unwrap_or(false);
                        if !alive && runtime.block_on(peripheral.connect()).is_err() {
                            None
                        } else {
                            Some(runtime.block_on(write_job(
                                peripheral,
                                characteristic,
                                &job,
                                fast,
                                cancel,
                                send,
                            )))
                        }
                    }
                };
                let Some(result) = result else {
                    send(BleEvent::Error(format!(
                        "lost the connection to {}; switch it on and connect again",
                        current.name
                    )));
                    session = None;
                    send(BleEvent::Disconnected);
                    continue;
                };
                match result {
                    Ok(true) => send(BleEvent::Printed(job.len())),
                    Ok(false) => send(BleEvent::Cancelled),
                    Err(error) => send(BleEvent::Error(error)),
                }
            }
            BleCommand::Disconnect => {
                if let Some(old) = session.take() {
                    runtime.block_on(old.disconnect());
                }
                send(BleEvent::Disconnected);
            }
        }
    }
}

impl Session {
    async fn disconnect(self) {
        if let Link::Gatt { peripheral, .. } = self.link {
            let _ = peripheral.disconnect().await;
        }
    }
}

fn connected_event(session: &Session) -> BleEvent {
    let (id, channel) = match &session.link {
        Link::Gatt {
            peripheral,
            characteristic,
        } => (
            format!("{:?}", peripheral.id()),
            characteristic.uuid.to_string(),
        ),
        Link::Serial(address) => (
            format!("{SERIAL_ID_PREFIX}{address}"),
            format!("Bluetooth Classic serial (RFCOMM) at {address}"),
        ),
    };
    BleEvent::Connected {
        id,
        name: session.name.clone(),
        channel,
    }
}

/// Checks that the printer answers on its Classic serial channel. The link
/// is closed again; each job opens its own.
fn open_serial(name: &str, address: &str) -> Result<Session, String> {
    tspl_core::rfcomm::connect(address)
        .map_err(|error| format!("{name} did not answer over Bluetooth Classic: {error}"))?;
    Ok(Session {
        link: Link::Serial(address.to_string()),
        name: name.to_string(),
    })
}

/// The Bluetooth Classic address to print to instead of LE: the device's own
/// when it is a Classic device, else that of its Classic twin (`NAME` for
/// `NAME-LE`) when the scan saw one.
#[cfg(target_os = "linux")]
async fn classic_address(
    adapter: &Adapter,
    properties: Option<&btleplug::api::PeripheralProperties>,
) -> Option<String> {
    let properties = properties?;
    if properties.class.is_some() {
        return Some(properties.address.to_string());
    }
    let name = properties.local_name.as_deref()?.to_ascii_lowercase();
    let twin = name.strip_suffix("-le")?;
    for candidate in adapter.peripherals().await.ok()? {
        let Some(other) = candidate.properties().await.ok().flatten() else {
            continue;
        };
        let same_name = other
            .local_name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(twin));
        if same_name && other.address != properties.address {
            return Some(other.address.to_string());
        }
    }
    None
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
    let mut found = hide_classic_twins(found);
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

    let properties = peripheral.properties().await.ok().flatten();
    let address = properties
        .as_ref()
        .map(|p| p.address.to_string())
        .unwrap_or_default();
    let name = properties
        .as_ref()
        .and_then(|p| p.local_name.clone())
        .unwrap_or_else(|| "Bluetooth printer".to_string());

    // Prefer the Classic serial channel on Linux (see the module docs). A
    // Classic device has no LE side to fall back to, so its error stands.
    #[cfg(target_os = "linux")]
    if let Some(classic) = classic_address(adapter, properties.as_ref()).await {
        match open_serial(&name, &classic) {
            Ok(session) => return Ok(session),
            Err(error) if properties.as_ref().is_some_and(|p| p.class.is_some()) => {
                return Err(error);
            }
            Err(_) => {}
        }
    }

    // On Linux `Device1.Connected` is true for any bearer and for a link this
    // app did not open: a desktop Bluetooth panel, BlueZ auto-connect, or a
    // failed Classic attempt that left the device connected. Nothing has waited
    // for `ServicesResolved` in that state, and `discover_services` only
    // introspects D-Bus, so the device looks connected with an empty GATT table.
    // Drop the link first so `connect` runs a full LE connect and waits for
    // service discovery. Windows never needed this and is unchanged.
    #[cfg(target_os = "linux")]
    if peripheral.is_connected().await.unwrap_or(false) {
        let _ = peripheral.disconnect().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // BlueZ on Linux sometimes abandons an LE connection while setting it up
    // ("le-connection-abort-by-local"); a second attempt usually succeeds.
    let mut attempt = 0;
    while !peripheral.is_connected().await.unwrap_or(false) {
        attempt += 1;
        match peripheral.connect().await {
            Ok(()) => break,
            Err(error) => {
                let error = error.to_string();
                if attempt < CONNECT_ATTEMPTS && is_transient_connect_error(&error) {
                    let _ = peripheral.disconnect().await;
                    tokio::time::sleep(CONNECT_RETRY_DELAY).await;
                    continue;
                }
                return Err(friendly_connect_error(&name, &address, &error));
            }
        }
    }

    // On Linux, BlueZ resolves services after the link is up. btleplug waits
    // for that inside connect(), but not when the link was already open (a
    // desktop Bluetooth panel, or an earlier attempt), so the first read can
    // come back empty. Read again a few times before giving up.
    let mut characteristics: Vec<Characteristic> = Vec::new();
    let mut infos: Vec<ChannelInfo> = Vec::new();
    let mut chosen = None;
    for attempt in 0..=DISCOVERY_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(DISCOVERY_RETRY_DELAY).await;
        }
        peripheral
            .discover_services()
            .await
            .map_err(|error| format!("read services of {name}: {error}"))?;
        characteristics = peripheral.characteristics().into_iter().collect();
        infos = characteristics.iter().map(channel_info).collect();
        chosen = choose_channel(&infos);
        if chosen.is_some() {
            break;
        }
    }

    let Some(index) = chosen else {
        let _ = peripheral.disconnect().await;
        return Err(channel_error(&name, &infos));
    };

    Ok(Session {
        link: Link::Gatt {
            characteristic: characteristics[index].clone(),
            peripheral,
        },
        name,
    })
}

fn channel_info(c: &Characteristic) -> ChannelInfo {
    ChannelInfo {
        uuid: c.uuid.to_string(),
        service: c.service_uuid.to_string(),
        write: c.properties.contains(CharPropFlags::WRITE),
        write_without_response: c.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE),
    }
}

/// Short summary of a GATT table for error messages and bug reports:
/// `services 1800, 49535343-fe7d-…; writable: 2a00`.
#[must_use]
pub fn describe_gatt(channels: &[ChannelInfo]) -> String {
    if channels.is_empty() {
        return "no services were reported".to_string();
    }
    let short = |uuid: &str| {
        let lower = uuid.to_ascii_lowercase();
        // 0000xxxx-0000-1000-8000-00805f9b34fb is a 16-bit Bluetooth SIG UUID.
        if lower.len() == 36
            && lower.starts_with("0000")
            && lower.ends_with("-0000-1000-8000-00805f9b34fb")
        {
            lower[4..8].to_string()
        } else {
            lower
        }
    };
    let mut services: Vec<String> = channels.iter().map(|c| short(&c.service)).collect();
    services.sort();
    services.dedup();
    let writable: Vec<String> = channels
        .iter()
        .filter(|c| c.write || c.write_without_response)
        .map(|c| short(&c.uuid))
        .collect();
    format!(
        "services {}; writable: {}",
        services.join(", "),
        if writable.is_empty() {
            "none".to_string()
        } else {
            writable.join(", ")
        }
    )
}

/// Explains a device that connected but left no usable print channel.
///
/// The two cases are genuinely different and used to share one message that
/// blamed Bluetooth Classic. An empty table means the stack never published
/// the printer's services at all (seen with BlueZ 5.88 on Fedora, where the
/// link comes up and the ATT service discovery is answered but no
/// characteristics are ever exported). A table with only standard services
/// means the device answered but has nothing that looks like a print channel.
#[must_use]
pub fn channel_error(name: &str, channels: &[ChannelInfo]) -> String {
    if channels.is_empty() {
        return format!(
            "{name} is connected, but the Bluetooth stack published no GATT services for it, \
             so nothing can be written. This is usually the operating system, not the printer: \
             on Linux, bluetoothd can leave a dual-mode printer connected with an empty GATT \
             table, and the printer did not answer over Bluetooth Classic either. Switch the \
             printer off and on, then connect again"
        );
    }
    format!(
        "{name} has no printer data channel ({}). Its writable characteristics are all part of \
         standard GATT services, so it may not be a label printer",
        describe_gatt(channels)
    )
}

/// Connection errors that are worth one more try: BlueZ dropped the link while
/// setting it up, or a previous attempt was still finishing.
#[must_use]
pub fn is_transient_connect_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "le-connection-abort-by-local",
        "connection-abort",
        "software caused connection abort",
        "in progress",
        "connection timed out",
        "br-connection-canceled",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

/// Turns BlueZ's connection errors into advice. `address` (may be empty) is
/// the printer's Bluetooth address, for the `bluetoothctl` command.
#[must_use]
pub fn friendly_connect_error(name: &str, address: &str, error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("no more profiles") || lower.contains("profile unavailable") {
        // BlueZ: a Bluetooth Classic device with nothing it can connect to.
        format!(
            "{name} is the printer's Bluetooth Classic side, which this app cannot use. \
             Connect to the entry whose name ends in -LE instead"
        )
    } else if is_transient_connect_error(error) {
        let forget = if address.is_empty() {
            "remove the printer in your Bluetooth settings".to_string()
        } else {
            format!("run: bluetoothctl remove {address}")
        };
        format!(
            "{name} dropped the connection while it was being set up ({error}). Make sure \
             no other computer or phone is connected to the printer, switch the printer off \
             and on, and try again. If it keeps happening, clear Linux's saved state for it: \
             {forget}, then scan again"
        )
    } else if lower.contains("page timeout") || lower.contains("host is down") {
        format!("{name} did not answer; check it is switched on and nearby, then try again")
    } else {
        format!("connect to {name}: {error}")
    }
}

/// Hides a device when another one in the list has the same name plus `-LE`.
///
/// Dual-mode printers advertise twice: `BY-482BT_F2CE` (Bluetooth Classic) and
/// `BY-482BT_F2CE-LE` (Bluetooth LE). Linux lists both; only the LE one works.
#[must_use]
pub fn hide_classic_twins(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    let le_names: Vec<String> = devices
        .iter()
        .filter_map(|d| {
            let lower = d.name.to_ascii_lowercase();
            lower.strip_suffix("-le").map(str::to_string)
        })
        .collect();
    devices
        .into_iter()
        .filter(|d| !le_names.contains(&d.name.to_ascii_lowercase()))
        .collect()
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

/// Writes `job` over a fresh RFCOMM link. Returns `Ok(false)` when cancelled.
/// The socket's own flow control paces the writes.
fn write_serial(
    address: &str,
    job: &[u8],
    cancel: &AtomicBool,
    send: &dyn Fn(BleEvent),
) -> Result<bool, String> {
    use std::io::Write as _;

    let mut link = tspl_core::rfcomm::connect(address).map_err(|error| error.to_string())?;
    let mut sent = 0;
    send(BleEvent::Progress {
        sent,
        total: job.len(),
    });
    for chunk in job.chunks(SERIAL_CHUNK) {
        if cancel.load(Ordering::SeqCst) {
            return Ok(false);
        }
        link.write_all(chunk)
            .map_err(|error| format!("Bluetooth write failed after {sent} bytes: {error}"))?;
        sent += chunk.len();
        send(BleEvent::Progress {
            sent,
            total: job.len(),
        });
    }
    Ok(true)
}

/// Writes `job` in chunks. Returns `Ok(false)` when cancelled.
async fn write_job(
    peripheral: &Peripheral,
    characteristic: &Characteristic,
    job: &[u8],
    fast: bool,
    cancel: &AtomicBool,
    send: &dyn Fn(BleEvent),
) -> Result<bool, String> {
    let properties = characteristic.properties;
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
        peripheral
            .write(characteristic, chunk, write_type)
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

    #[test]
    fn classic_twins_are_hidden_but_other_devices_stay() {
        let devices = vec![
            device("a", "BY-482BT_F2CE"),
            device("b", "BY-482BT_F2CE-LE"),
            device("c", "Soundcore"),
            device("d", "Printer-LE-Edition"),
        ];
        let names: Vec<String> = hide_classic_twins(devices)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(
            names,
            ["BY-482BT_F2CE-LE", "Soundcore", "Printer-LE-Edition"]
        );

        // A Classic name with no LE twin is kept, and case does not matter.
        let names: Vec<String> = hide_classic_twins(vec![
            device("a", "by-482bt_f2ce"),
            device("b", "BY-482BT_F2CE-le"),
        ])
        .into_iter()
        .map(|d| d.name)
        .collect();
        assert_eq!(names, ["BY-482BT_F2CE-le"]);
        assert_eq!(
            hide_classic_twins(vec![device("a", "BY-482BT_F2CE")]).len(),
            1
        );
    }

    #[test]
    fn bluez_errors_become_advice() {
        let message = friendly_connect_error(
            "BY-482BT_F2CE",
            "",
            "org.bluez.Error.Failed: No more profiles to connect to",
        );
        assert!(message.contains("-LE"), "{message}");

        let message = friendly_connect_error(
            "BY-482BT_F2CE-LE",
            "DC:0D:30:87:F2:CE",
            "org.bluez.Error.Failed: le-connection-abort-by-local",
        );
        assert!(
            message.contains("bluetoothctl remove DC:0D:30:87:F2:CE"),
            "{message}"
        );
        assert!(
            message.contains("le-connection-abort-by-local"),
            "{message}"
        );
        let without_address = friendly_connect_error("X", "", "le-connection-abort-by-local");
        assert!(
            without_address.contains("Bluetooth settings"),
            "{without_address}"
        );

        let message = friendly_connect_error("X", "", "Page Timeout");
        assert!(message.contains("switched on"), "{message}");
        assert_eq!(
            friendly_connect_error("X", "", "boom"),
            "connect to X: boom"
        );
    }

    #[test]
    fn only_transient_errors_are_retried() {
        for retry in [
            "org.bluez.Error.Failed: le-connection-abort-by-local",
            "Software caused connection abort",
            "org.bluez.Error.InProgress: In Progress",
        ] {
            assert!(is_transient_connect_error(retry), "{retry}");
        }
        for give_up in [
            "No more profiles to connect to",
            "Page Timeout",
            "Not authorized",
            "",
        ] {
            assert!(!is_transient_connect_error(give_up), "{give_up}");
        }
    }

    #[test]
    fn empty_and_standard_only_tables_get_different_advice() {
        // Connected, but the stack published nothing: blame the stack, not the printer.
        let message = channel_error("BY-482BT_F2CE-LE", &[]);
        assert!(message.contains("published no GATT services"), "{message}");
        assert!(message.contains("Bluetooth Classic"), "{message}");
        assert!(!message.contains("may not be a label printer"), "{message}");

        // A real table with only standard services: the device answered, but has
        // nothing writable that could carry TSPL.
        let channels = [channel(DEVICE_NAME, GAP, true, false)];
        let message = channel_error("Speaker", &channels);
        assert!(message.contains("no printer data channel"), "{message}");
        assert!(
            message.contains("writable: 2a00"),
            "short UUID in {message}"
        );
        assert!(!message.contains("published no GATT services"), "{message}");
    }

    #[test]
    fn gatt_summaries_shorten_standard_uuids() {
        let channels = [
            channel(DEVICE_NAME, GAP, true, false),
            channel(
                "49535343-8841-43f4-a8d4-ecbe34729bb3",
                "49535343-fe7d-4ae5-8fa9-9fafd205e455",
                true,
                true,
            ),
            channel(
                "00002a29-0000-1000-8000-00805f9b34fb",
                "0000180a-0000-1000-8000-00805f9b34fb",
                false,
                false,
            ),
        ];
        assert_eq!(
            describe_gatt(&channels),
            "services 1800, 180a, 49535343-fe7d-4ae5-8fa9-9fafd205e455; \
             writable: 2a00, 49535343-8841-43f4-a8d4-ecbe34729bb3"
        );
        assert_eq!(describe_gatt(&[]), "no services were reported");
    }
}
