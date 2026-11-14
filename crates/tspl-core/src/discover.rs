//! Printer discovery: spooler queues, USB printers and serial / Bluetooth ports.
//!
//! Discovery never fails as a whole. Each source contributes printers or a
//! warning, so a UI can show "queues unavailable" next to a working USB list
//! instead of an error page.
//!
//! Discovery is split in two so the interesting part is testable anywhere:
//!
//! 1. Platform code gathers raw facts ([`QueueInfo`], [`UsbDevice`],
//!    [`SerialDevice`]).
//! 2. [`assemble`] turns them into [`DiscoveredPrinter`]s: names them, links a
//!    queue to the USB device behind its port (so an unplugged printer shows as
//!    offline), drops duplicates and noise, and decides which ones are label
//!    printers. [`best_printer`] then picks what to connect to automatically.

pub use crate::spooler::QueueInfo;

/// Where a discovered printer is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrinterKind {
    /// A spooler queue (Windows spooler or CUPS), printed to as a RAW job.
    Queue,
    /// A USB printer written to directly (`usbprint.sys` or `/dev/usb/lp*`).
    Usb,
    /// A Bluetooth Classic printer paired as a serial port (SPP).
    BluetoothSerial,
    /// Any other serial port or device node.
    Serial,
}

impl PrinterKind {
    /// Short label for the connection type.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Queue => "Print queue",
            Self::Usb => "USB",
            Self::BluetoothSerial => "Bluetooth",
            Self::Serial => "Serial port",
        }
    }
}

/// One printer the application can offer to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPrinter {
    /// Display name: the printer model when known, otherwise the queue or port.
    pub name: String,
    /// How to reach it.
    pub kind: PrinterKind,
    /// Queue name, device path or port name.
    pub address: String,
    /// Short human-readable note, e.g. `USB001 · Generic / Text Only`.
    pub detail: String,
    /// Manufacturer and model when the device reported them.
    pub model: Option<String>,
    /// True when name, model or command set say this is a label printer.
    pub label_printer: bool,
    /// False when the device behind a queue is known to be unplugged.
    pub online: bool,
    /// Present but not worth showing by default: virtual queues (PDF, XPS,
    /// fax), duplicates of a queue's USB device, ports that are not printers.
    pub noise: bool,
}

impl DiscoveredPrinter {
    /// The `--target` value that addresses this printer.
    #[must_use]
    pub fn target_spec(&self) -> String {
        match self.kind {
            PrinterKind::Queue => format!("raw:{}", self.address),
            PrinterKind::Usb => format!("usb:{}", self.address),
            PrinterKind::BluetoothSerial | PrinterKind::Serial => {
                format!("serial:{}", self.address)
            }
        }
    }

    /// Preference for automatic connection; `0` means never pick automatically.
    ///
    /// Only online label printers qualify. Direct USB wins: it involves no
    /// driver and no queue at all, just `usbprint.sys` (or `usblp`). A spooler
    /// queue still works but depends on a driver being installed; Bluetooth
    /// is slower; a generic serial port is a last resort.
    #[must_use]
    pub fn auto_rank(&self) -> u32 {
        if !self.online || self.noise || !self.label_printer {
            return 0;
        }
        match self.kind {
            PrinterKind::Usb => 100,
            PrinterKind::Queue if self.model.is_some() => 80,
            PrinterKind::Queue => 70,
            PrinterKind::BluetoothSerial => 60,
            PrinterKind::Serial => 40,
        }
    }
}

/// Result of a discovery sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    /// Printers found, best candidates first.
    pub printers: Vec<DiscoveredPrinter>,
    /// Sources that could not be queried.
    pub warnings: Vec<String>,
}

/// A USB printer as reported by the platform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsbDevice {
    /// Device path to write to.
    pub path: String,
    /// Spooler port the device is bound to (`USB001`), when known.
    pub port: Option<String>,
    /// USB product string.
    pub product: Option<String>,
    /// Raw IEEE 1284 device id.
    pub device_id: Option<String>,
}

/// A serial port as reported by the platform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SerialDevice {
    /// `COM5` or `/dev/rfcomm0`.
    pub port: String,
    /// Friendly name, e.g. `Standard Serial over Bluetooth link (COM5)`.
    pub friendly: Option<String>,
    /// `PnP` instance id (Windows), used to find the Bluetooth address.
    pub instance_id: Option<String>,
    /// Name of the paired Bluetooth device behind the port.
    pub bluetooth_name: Option<String>,
}

/// Fields of an IEEE 1284 device id that matter here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceId {
    /// `MFG` / `MANUFACTURER`.
    pub manufacturer: Option<String>,
    /// `MDL` / `MODEL`.
    pub model: Option<String>,
    /// `CMD` / `COMMAND SET`, e.g. `TSPL`.
    pub command_set: Option<String>,
}

impl DeviceId {
    /// `PEDOOLO BY-482BT`, without repeating a manufacturer already in the model.
    #[must_use]
    pub fn display(&self) -> Option<String> {
        match (&self.manufacturer, &self.model) {
            (Some(make), Some(model))
                if !model
                    .to_ascii_lowercase()
                    .starts_with(&make.to_ascii_lowercase()) =>
            {
                Some(format!("{make} {model}"))
            }
            (_, Some(model)) => Some(model.clone()),
            (Some(make), None) => Some(make.clone()),
            (None, None) => None,
        }
    }

    /// True when the command set is a label language (TSPL, ZPL, EPL, …).
    #[must_use]
    pub fn speaks_label_language(&self) -> bool {
        self.command_set.as_deref().is_some_and(|commands| {
            let commands = commands.to_ascii_uppercase();
            ["TSPL", "ZPL", "EPL", "DPL", "CPCL"]
                .iter()
                .any(|language| commands.contains(language))
        })
    }
}

/// Names that identify label printers. Matched case-insensitively as
/// substrings of names, models and product strings.
const LABEL_PRINTER_HINTS: &[&str] = &[
    "by-482", "by482", "beeprt", "pedoolo", "rhebar", "ip-592", "tspl", "label", "thermal",
    "phomemo", "munbyn", "rollo", "itari", "jadens", "idprt", "xprinter", "gprinter", "hprt",
    "zebra", "zd4", "zd2", "dymo", "niimbot", "m110", "m220", "pm-241", "4barcode", "shipping",
];

/// Queues that are documents, not devices.
const VIRTUAL_QUEUE_HINTS: &[&str] = &[
    "microsoft print to pdf",
    "microsoft xps",
    "onenote",
    "fax",
    "pdf",
    "anydesk",
    "send to",
];

/// Ports that never lead to hardware.
const VIRTUAL_PORTS: &[&str] = &["portprompt:", "nul:", "file:", "xpsport:", "shrfax:"];

/// Enumerates every source and assembles the result.
#[must_use]
pub fn discover() -> Discovery {
    let mut warnings = Vec::new();

    let queues = match crate::spooler::list_queue_details() {
        Ok(queues) => queues,
        Err(error) => {
            warnings.push(format!("print queues: {error}"));
            Vec::new()
        }
    };

    let usb = match usb_devices() {
        Ok(devices) => devices,
        Err(error) => {
            warnings.push(format!("USB printers: {error}"));
            Vec::new()
        }
    };

    let serial = match serial_devices() {
        Ok(ports) => ports,
        Err(error) => {
            warnings.push(format!("serial ports: {error}"));
            Vec::new()
        }
    };

    Discovery {
        printers: assemble(&queues, &usb, &serial),
        warnings,
    }
}

/// Lists spooler queue names.
///
/// # Errors
///
/// Propagates the platform error from the spooler.
pub fn list_queues() -> crate::Result<Vec<String>> {
    crate::spooler::list_queues()
}

/// Lists serial port names.
///
/// # Errors
///
/// Returns [`crate::Error::Io`] when the port list cannot be read.
pub fn list_serial_ports() -> crate::Result<Vec<String>> {
    Ok(serial_devices()?
        .into_iter()
        .map(|port| port.port)
        .collect())
}

/// USB printers from the platform layer.
///
/// # Errors
///
/// Returns the platform error when the device list cannot be read.
pub fn usb_devices() -> crate::Result<Vec<UsbDevice>> {
    #[cfg(windows)]
    {
        Ok(crate::os::usb_printers()?
            .into_iter()
            .map(|device| UsbDevice {
                path: device.path,
                port: device.port,
                product: device.product,
                device_id: device.device_id,
            })
            .collect())
    }
    #[cfg(not(windows))]
    {
        Ok(crate::os::usb_printers()?
            .into_iter()
            .map(|device| UsbDevice {
                path: device.path,
                port: None,
                product: None,
                device_id: device.device_id,
            })
            .collect())
    }
}

/// Serial ports from the platform layer.
///
/// # Errors
///
/// Returns the platform error when the port list cannot be read.
pub fn serial_devices() -> crate::Result<Vec<SerialDevice>> {
    #[cfg(windows)]
    {
        Ok(crate::os::serial_ports()?
            .into_iter()
            .map(|port| SerialDevice {
                port: port.port,
                friendly: Some(port.friendly),
                instance_id: Some(port.instance_id),
                bluetooth_name: port.bluetooth_name,
            })
            .collect())
    }
    #[cfg(not(windows))]
    {
        Ok(crate::os::serial_ports()?
            .into_iter()
            .map(|port| SerialDevice {
                bluetooth_name: port
                    .contains("rfcomm")
                    .then(|| "Bluetooth serial".to_string()),
                port,
                friendly: None,
                instance_id: None,
            })
            .collect())
    }
}

/// Turns raw platform facts into the printer list, best candidates first.
#[must_use]
pub fn assemble(
    queues: &[QueueInfo],
    usb: &[UsbDevice],
    serial: &[SerialDevice],
) -> Vec<DiscoveredPrinter> {
    let mut printers = Vec::new();

    for queue in queues {
        printers.push(queue_printer(queue, usb));
    }

    for device in usb {
        let id = device
            .device_id
            .as_deref()
            .map(parse_device_id)
            .unwrap_or_default();
        let model = id.display().or_else(|| device.product.clone());

        let label_printer =
            id.speaks_label_language() || model.as_deref().is_some_and(looks_like_label_printer);

        let mut detail = String::from("Direct USB · no driver");
        if let Some(commands) = &id.command_set {
            detail = format!("{detail} · {commands}");
        }
        if let Some(port) = &device.port {
            detail = format!("{detail} · {port}");
        }

        printers.push(DiscoveredPrinter {
            name: model.clone().unwrap_or_else(|| "USB printer".to_string()),
            kind: PrinterKind::Usb,
            address: device.path.clone(),
            detail,
            model,
            label_printer,
            online: true,
            noise: false,
        });
    }

    for port in serial {
        let address = port.instance_id.as_deref().and_then(bluetooth_address);
        let is_bluetooth = port.bluetooth_name.is_some()
            || address.is_some()
            || port
                .instance_id
                .as_deref()
                .is_some_and(|id| id.to_ascii_uppercase().starts_with("BTHENUM"));

        // Incoming SPP ports carry an all-zero address: they wait for a phone
        // to call in and can never reach a printer.
        if is_bluetooth
            && port.instance_id.as_deref().is_some_and(|id| {
                raw_bluetooth_address(id).is_some_and(|a| a.chars().all(|c| c == '0'))
            })
        {
            continue;
        }

        let friendly = port.friendly.clone().unwrap_or_else(|| port.port.clone());
        let (name, kind, detail) = if is_bluetooth {
            let name = port
                .bluetooth_name
                .clone()
                .unwrap_or_else(|| "Bluetooth device".to_string());
            let detail = match &address {
                Some(address) => format!("Paired · {} · {}", port.port, format_address(address)),
                None => format!("Paired · {}", port.port),
            };
            (name, PrinterKind::BluetoothSerial, detail)
        } else {
            (friendly.clone(), PrinterKind::Serial, port.port.clone())
        };

        let label_printer = looks_like_label_printer(&name)
            || (is_bluetooth && name.to_ascii_lowercase().contains("print"));

        printers.push(DiscoveredPrinter {
            name,
            kind,
            address: port.port.clone(),
            detail,
            model: None,
            label_printer,
            online: true,
            noise: !label_printer,
        });
    }

    printers.sort_by(|a, b| {
        b.auto_rank()
            .cmp(&a.auto_rank())
            .then(a.noise.cmp(&b.noise))
            .then(b.label_printer.cmp(&a.label_printer))
            .then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
    });
    printers
}

fn queue_printer(queue: &QueueInfo, usb: &[UsbDevice]) -> DiscoveredPrinter {
    let port = queue.port.clone().unwrap_or_default();
    let linked = usb.iter().find(|device| {
        device
            .port
            .as_deref()
            .is_some_and(|p| !port.is_empty() && p.eq_ignore_ascii_case(&port))
    });
    let id = linked
        .and_then(|device| device.device_id.as_deref())
        .map(parse_device_id)
        .unwrap_or_default();
    let model = id
        .display()
        .or_else(|| linked.and_then(|device| device.product.clone()));

    // A Windows USB port with no device behind it means the printer is
    // unplugged or switched off. Other ports (network, CUPS URIs) cannot be
    // checked cheaply, so they are assumed reachable.
    let online = if is_windows_usb_port(&port) {
        linked.is_some()
    } else {
        true
    };

    let is_virtual = is_virtual_queue(&queue.name, queue.port.as_deref(), queue.driver.as_deref());
    let label_printer = id.speaks_label_language()
        || looks_like_label_printer(&queue.name)
        || queue
            .driver
            .as_deref()
            .is_some_and(looks_like_label_printer)
        || model.as_deref().is_some_and(looks_like_label_printer);

    let mut parts = Vec::new();
    if !port.is_empty() {
        parts.push(port.clone());
    }
    if let Some(driver) = &queue.driver {
        parts.push(driver.clone());
    }
    if is_windows_usb_port(&port) && !online {
        parts.push("printer not connected".to_string());
    }

    DiscoveredPrinter {
        name: queue.name.clone(),
        kind: PrinterKind::Queue,
        address: queue.name.clone(),
        detail: parts.join(" · "),
        model,
        label_printer,
        online,
        // A queue whose USB device is present is a duplicate of the direct
        // USB entry, which needs no driver; show the printer once.
        noise: is_virtual || linked.is_some(),
    }
}

/// Picks the printer to connect to without asking.
///
/// The `preferred` target spec (the user's last choice) wins whenever it is
/// present and online, even if it does not look like a label printer — the
/// user chose it. Otherwise the highest [`DiscoveredPrinter::auto_rank`] wins;
/// nothing is chosen when no online label printer exists, so the app never
/// sends TSPL to an office laser printer on its own.
#[must_use]
pub fn best_printer<'a>(
    printers: &'a [DiscoveredPrinter],
    preferred: Option<&str>,
) -> Option<&'a DiscoveredPrinter> {
    if let Some(spec) = preferred {
        if let Some(printer) = printers
            .iter()
            .find(|printer| printer.online && printer.target_spec().eq_ignore_ascii_case(spec))
        {
            return Some(printer);
        }
    }

    printers
        .iter()
        .filter(|printer| printer.auto_rank() > 0)
        .max_by(|a, b| {
            a.auto_rank()
                .cmp(&b.auto_rank())
                // Prefer the alphabetically first among equals, deterministically.
                .then_with(|| b.name.cmp(&a.name))
        })
}

/// True when `text` names a known label printer brand, model or language.
#[must_use]
pub fn looks_like_label_printer(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    LABEL_PRINTER_HINTS.iter().any(|hint| lower.contains(hint))
}

/// True for queues that produce files rather than paper.
#[must_use]
pub fn is_virtual_queue(name: &str, port: Option<&str>, driver: Option<&str>) -> bool {
    let name = name.to_ascii_lowercase();
    let driver = driver.unwrap_or_default().to_ascii_lowercase();
    let port = port.unwrap_or_default().to_ascii_lowercase();

    VIRTUAL_PORTS.contains(&port.as_str())
        || VIRTUAL_QUEUE_HINTS
            .iter()
            .any(|hint| name.contains(hint) || driver.contains(hint))
}

/// `USB001`, `USB002`, … — ports created by the Windows USB port monitor.
fn is_windows_usb_port(port: &str) -> bool {
    let upper = port.to_ascii_uppercase();
    upper.len() > 3 && upper.starts_with("USB") && upper[3..].chars().all(|c| c.is_ascii_digit())
}

/// Parses an IEEE 1284 device id such as `MFG:PEDOOLO;MDL:BY-482BT;CMD:TSPL;`.
#[must_use]
pub fn parse_device_id(text: &str) -> DeviceId {
    let mut id = DeviceId::default();
    for field in text.split(';') {
        let Some((key, value)) = field.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim().to_ascii_uppercase().as_str() {
            "MFG" | "MANUFACTURER" => id.manufacturer = Some(value.to_string()),
            "MDL" | "MODEL" => id.model = Some(value.to_string()),
            "CMD" | "COMMAND SET" | "COMMANDSET" => id.command_set = Some(value.to_string()),
            _ => {}
        }
    }
    id
}

/// Decodes the reply to `IOCTL_USBPRINT_GET_1284_ID` (and the identical
/// `LPIOC_GET_DEVICE_ID` layout): a big-endian length that counts itself,
/// followed by the ASCII id.
#[must_use]
pub fn decode_1284_reply(reply: &[u8]) -> Option<String> {
    if reply.len() < 3 {
        return None;
    }
    let declared = usize::from(u16::from_be_bytes([reply[0], reply[1]]));
    let end = if (3..=reply.len()).contains(&declared) {
        declared
    } else {
        reply.len()
    };
    let body = &reply[2..end];
    let body = body.split(|&b| b == 0).next().unwrap_or(body);
    let text = String::from_utf8_lossy(body).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The 12-hex-digit remote address in a `BTHENUM` instance id, or `None` for
/// incoming ports (all zeros) and non-Bluetooth devices.
///
/// `BTHENUM\{00001101-…}_LOCALMFG&005D\8&13083845&0&E8EECC06FE6E_C00000000`
/// yields `E8EECC06FE6E`.
#[must_use]
pub fn bluetooth_address(instance_id: &str) -> Option<String> {
    raw_bluetooth_address(instance_id).filter(|address| address.chars().any(|c| c != '0'))
}

fn raw_bluetooth_address(instance_id: &str) -> Option<String> {
    if !instance_id.to_ascii_uppercase().starts_with("BTHENUM") {
        return None;
    }
    let last = instance_id.rsplit('\\').next()?;
    let candidate = last.rsplit('&').next()?.split('_').next()?;
    (candidate.len() == 12 && candidate.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| candidate.to_ascii_uppercase())
}

/// `E8EECC06FE6E` as `E8:EE:CC:06:FE:6E`.
fn format_address(address: &str) -> String {
    address
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(":")
}

/// True for `/dev` entries that can reach a printer.
///
/// macOS `tty.*` nodes are the dial-in twins of `cu.*` and block on open;
/// the console and incoming-Bluetooth nodes exist on every Mac.
#[must_use]
pub fn is_useful_serial_node(name: &str) -> bool {
    let wanted = name.starts_with("cu.")
        || name.starts_with("ttyUSB")
        || name.starts_with("ttyACM")
        || name.starts_with("rfcomm");
    let noise = name.contains("Bluetooth-Incoming-Port") || name.contains("debug-console");
    wanted && !noise
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(name: &str, port: &str, driver: &str) -> QueueInfo {
        QueueInfo {
            name: name.to_string(),
            port: Some(port.to_string()),
            driver: Some(driver.to_string()),
        }
    }

    fn by482_usb(port: Option<&str>) -> UsbDevice {
        UsbDevice {
            path: r"\\?\usb#vid_09c6&pid_0426#482b11242604#{28d78fad-5a12-11d1-ae5b-0000f803a8c2}"
                .to_string(),
            port: port.map(ToString::to_string),
            product: Some("BY-482BT".to_string()),
            device_id: Some("MFG:PEDOOLO;MDL:BY-482BT;CMD:TSPL;CLS:PRINTER;".to_string()),
        }
    }

    fn bt_port(port: &str, address: &str, name: Option<&str>) -> SerialDevice {
        SerialDevice {
            port: port.to_string(),
            friendly: Some(format!("Standard Serial over Bluetooth link ({port})")),
            instance_id: Some(format!(
                r"BTHENUM\{{00001101-0000-1000-8000-00805F9B34FB}}_LOCALMFG&005D\8&13083845&0&{address}_C00000000"
            )),
            bluetooth_name: name.map(ToString::to_string),
        }
    }

    #[test]
    fn device_ids_parse_with_aliases_and_noise() {
        let cases = [
            (
                "MFG:PEDOOLO;MDL:BY-482BT;CMD:TSPL;",
                Some("PEDOOLO"),
                Some("BY-482BT"),
                Some("TSPL"),
            ),
            (
                "MANUFACTURER:Zebra;COMMAND SET:ZPL;MODEL:ZD420;",
                Some("Zebra"),
                Some("ZD420"),
                Some("ZPL"),
            ),
            ("garbage;;:;MDL:;", None, None, None),
            ("", None, None, None),
        ];
        for (input, make, model, cmd) in cases {
            let id = parse_device_id(input);
            assert_eq!(id.manufacturer.as_deref(), make, "{input}");
            assert_eq!(id.model.as_deref(), model, "{input}");
            assert_eq!(id.command_set.as_deref(), cmd, "{input}");
        }
    }

    #[test]
    fn device_id_display_does_not_repeat_the_maker() {
        let id = parse_device_id("MFG:Zebra;MDL:Zebra ZD420;");
        assert_eq!(id.display().as_deref(), Some("Zebra ZD420"));
        let id = parse_device_id("MFG:PEDOOLO;MDL:BY-482BT;");
        assert_eq!(id.display().as_deref(), Some("PEDOOLO BY-482BT"));
        assert!(parse_device_id("CMD:TSPL;").speaks_label_language());
        assert!(!parse_device_id("CMD:PCL,PJL;").speaks_label_language());
    }

    #[test]
    fn ieee1284_reply_decoding() {
        let mut reply = vec![0, 0];
        reply.extend_from_slice(b"MFG:X;MDL:Y;");
        let len = u16::try_from(reply.len()).unwrap();
        reply[..2].copy_from_slice(&len.to_be_bytes());
        reply.extend_from_slice(&[0, 0, 0]); // trailing buffer slack
        assert_eq!(decode_1284_reply(&reply).as_deref(), Some("MFG:X;MDL:Y;"));

        // A wrong length falls back to the whole buffer, NUL-terminated.
        assert_eq!(
            decode_1284_reply(b"\xff\xffMDL:Z;\0junk").as_deref(),
            Some("MDL:Z;")
        );
        assert_eq!(decode_1284_reply(b"\x00\x02"), None);
        assert_eq!(decode_1284_reply(b""), None);
    }

    #[test]
    fn bluetooth_addresses_come_from_the_instance_id() {
        let outgoing = r"BTHENUM\{00001101-0000-1000-8000-00805F9B34FB}_LOCALMFG&005D\8&13083845&0&E8EECC06FE6E_C00000000";
        let incoming = r"BTHENUM\{00001101-0000-1000-8000-00805F9B34FB}_LOCALMFG&0000\8&13083845&0&000000000000_00000000";
        assert_eq!(bluetooth_address(outgoing).as_deref(), Some("E8EECC06FE6E"));
        assert_eq!(bluetooth_address(incoming), None);
        assert_eq!(
            bluetooth_address(r"USB\VID_28DE&PID_1304&MI_00\7&1C5F3C11&0&0000"),
            None
        );
        assert_eq!(bluetooth_address(r"ACPI\PNP0501\0"), None);
        assert_eq!(bluetooth_address(""), None);
    }

    #[test]
    fn a_queue_on_a_present_usb_port_is_online_and_named() {
        let printers = assemble(
            &[queue("BY-482BT-RAW", "USB001", "Generic / Text Only")],
            &[by482_usb(Some("USB001"))],
            &[],
        );

        // Direct USB comes first: it needs no driver.
        let usb = &printers[0];
        assert_eq!(usb.kind, PrinterKind::Usb);
        assert!(!usb.noise);
        assert_eq!(usb.auto_rank(), 100);

        // The queue is online and named, but hidden as a duplicate.
        let queue = printers
            .iter()
            .find(|p| p.kind == PrinterKind::Queue)
            .expect("queue entry");
        assert!(queue.online);
        assert!(queue.label_printer);
        assert!(queue.noise);
        assert_eq!(queue.model.as_deref(), Some("PEDOOLO BY-482BT"));
    }

    #[test]
    fn a_queue_whose_usb_printer_is_unplugged_is_offline() {
        let printers = assemble(
            &[queue("BY-482BT-RAW", "USB001", "Generic / Text Only")],
            &[],
            &[],
        );
        assert!(!printers[0].online);
        assert!(printers[0].detail.contains("not connected"));
        assert_eq!(printers[0].auto_rank(), 0);
        assert!(best_printer(&printers, None).is_none());
        // Not even a remembered choice is used while it is offline.
        assert!(best_printer(&printers, Some("raw:BY-482BT-RAW")).is_none());
    }

    #[test]
    fn a_usb_printer_without_a_queue_is_offered_directly() {
        let printers = assemble(&[], &[by482_usb(None)], &[]);
        assert_eq!(printers.len(), 1);
        assert_eq!(printers[0].kind, PrinterKind::Usb);
        assert_eq!(printers[0].name, "PEDOOLO BY-482BT");
        assert!(printers[0].target_spec().starts_with("usb:"));
        assert!(!printers[0].noise);
        assert_eq!(
            best_printer(&printers, None).map(|p| p.kind),
            Some(PrinterKind::Usb)
        );
    }

    #[test]
    fn virtual_queues_and_non_printer_ports_are_noise() {
        let printers = assemble(
            &[
                queue(
                    "Microsoft Print to PDF",
                    "PORTPROMPT:",
                    "Microsoft Print To PDF",
                ),
                queue("Fax", "SHRFAX:", "Microsoft Shared Fax Driver"),
            ],
            &[],
            &[
                SerialDevice {
                    port: "COM1".to_string(),
                    friendly: Some("Communications Port (COM1)".to_string()),
                    instance_id: Some(r"ACPI\PNP0501\0".to_string()),
                    bluetooth_name: None,
                },
                bt_port("COM6", "E8EECC06FE6E", Some("Soundcore Liberty 3 Pro")),
            ],
        );
        assert!(printers.iter().all(|p| p.noise), "{printers:#?}");
        assert!(best_printer(&printers, None).is_none());
    }

    #[test]
    fn incoming_bluetooth_ports_are_dropped() {
        let printers = assemble(&[], &[], &[bt_port("COM7", "000000000000", None)]);
        assert!(printers.is_empty(), "{printers:#?}");
    }

    #[test]
    fn a_paired_bluetooth_printer_is_named_and_ranked() {
        let printers = assemble(
            &[],
            &[],
            &[
                bt_port("COM6", "E8EECC06FE6E", Some("Soundcore Liberty 3 Pro")),
                bt_port("COM8", "DC0D30AA1122", Some("BY-482BT")),
            ],
        );
        let best = best_printer(&printers, None).expect("a printer");
        assert_eq!(best.name, "BY-482BT");
        assert_eq!(best.kind, PrinterKind::BluetoothSerial);
        assert_eq!(best.target_spec(), "serial:COM8");
        assert!(best.detail.contains("DC:0D:30:AA:11:22"));
        // Best candidates sort first.
        assert_eq!(printers[0].name, "BY-482BT");
    }

    #[test]
    fn wired_beats_bluetooth_and_preference_beats_both() {
        let printers = assemble(
            &[queue("BY-482BT-RAW", "USB001", "Generic / Text Only")],
            &[by482_usb(Some("USB001"))],
            &[bt_port("COM8", "DC0D30AA1122", Some("BY-482BT"))],
        );
        assert_eq!(
            best_printer(&printers, None).map(DiscoveredPrinter::target_spec),
            Some(format!("usb:{}", by482_usb(None).path))
        );
        assert_eq!(
            best_printer(&printers, Some("SERIAL:com8")).map(DiscoveredPrinter::target_spec),
            Some("serial:COM8".to_string())
        );
        // A stale preference falls back to ranking.
        assert_eq!(
            best_printer(&printers, Some("serial:COM99")).map(DiscoveredPrinter::target_spec),
            Some(format!("usb:{}", by482_usb(None).path))
        );
    }

    #[test]
    fn a_users_choice_is_honoured_even_for_a_non_label_queue() {
        let printers = assemble(&[queue("Office Laser", "192.168.1.9", "HP PCL6")], &[], &[]);
        assert!(best_printer(&printers, None).is_none());
        assert_eq!(
            best_printer(&printers, Some("raw:Office Laser")).map(|p| p.name.as_str()),
            Some("Office Laser")
        );
    }

    #[test]
    fn label_printer_heuristics() {
        for yes in [
            "BY-482BT",
            "Beeprt label",
            "PEDOOLO",
            "Zebra ZD421",
            "Phomemo M110",
            "TSPL",
        ] {
            assert!(looks_like_label_printer(yes), "{yes}");
        }
        for no in [
            "Soundcore Liberty 3 Pro",
            "HP LaserJet",
            "Communications Port (COM1)",
            "",
        ] {
            assert!(!looks_like_label_printer(no), "{no}");
        }
    }

    #[test]
    fn virtual_queue_detection() {
        assert!(is_virtual_queue(
            "Microsoft Print to PDF",
            Some("PORTPROMPT:"),
            None
        ));
        assert!(is_virtual_queue("Anything", Some("nul:"), None));
        assert!(is_virtual_queue(
            "OneNote (Desktop)",
            Some("nul:"),
            Some("Send to Microsoft OneNote 16 Driver")
        ));
        assert!(!is_virtual_queue(
            "BY-482BT-RAW",
            Some("USB001"),
            Some("Generic / Text Only")
        ));
        assert!(!is_virtual_queue("Office", None, None));
    }

    #[test]
    fn usb_port_names() {
        assert!(is_windows_usb_port("USB001"));
        assert!(is_windows_usb_port("usb012"));
        assert!(!is_windows_usb_port("USB"));
        assert!(!is_windows_usb_port("USBPRINT"));
        assert!(!is_windows_usb_port("usb://PEDOOLO/BY-482BT"));
    }

    #[test]
    fn unix_serial_node_filter() {
        for yes in [
            "cu.usbserial-110",
            "ttyUSB0",
            "ttyACM1",
            "rfcomm0",
            "cu.BY-482BT",
        ] {
            assert!(is_useful_serial_node(yes), "{yes}");
        }
        for no in [
            "tty.usbserial-110",
            "cu.Bluetooth-Incoming-Port",
            "cu.debug-console",
            "tty0",
            "null",
        ] {
            assert!(!is_useful_serial_node(no), "{no}");
        }
    }
}
