//! Windows device enumeration and direct USB printer I/O.
//!
//! Two facts make driverless discovery possible on Windows:
//!
//! * Every USB printer-class device is bound to the inbox `usbprint.sys`,
//!   which publishes a device interface (`GUID_DEVINTERFACE_USBPRINT`). That
//!   interface can be opened with `CreateFileW` and written with `WriteFile` —
//!   no queue, no driver, no administrator rights. It also answers
//!   `IOCTL_USBPRINT_GET_1284_ID` with the printer's IEEE 1284 identity
//!   (`MFG:…;MDL:…;CMD:TSPL;`), which tells us what we are talking to.
//! * Paired Bluetooth Classic printers appear as "Standard Serial over
//!   Bluetooth link" COM ports. The device instance id carries the remote
//!   address, and the Bluetooth stack stores the paired device's name under
//!   `BTHPORT\Parameters\Devices\<address>`.
//!
//! Only `SetupAPI`, the registry and kernel32 are used, declared by hand like the
//! spooler bindings, so the crate keeps zero dependencies.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, OnceLock};

use crate::error::{Error, Result};

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// `GUID_DEVINTERFACE_USBPRINT` {28d78fad-5a12-11d1-ae5b-0000f803a8c2}.
const GUID_DEVINTERFACE_USBPRINT: Guid = Guid {
    data1: 0x28d7_8fad,
    data2: 0x5a12,
    data3: 0x11d1,
    data4: [0xae, 0x5b, 0x00, 0x00, 0xf8, 0x03, 0xa8, 0xc2],
};

/// `GUID_DEVCLASS_PORTS` {4d36e978-e325-11ce-bfc1-08002be10318}.
const GUID_DEVCLASS_PORTS: Guid = Guid {
    data1: 0x4d36_e978,
    data2: 0xe325,
    data3: 0x11ce,
    data4: [0xbf, 0xc1, 0x08, 0x00, 0x2b, 0xe1, 0x03, 0x18],
};

/// `DEVPKEY_Device_BusReportedDeviceDesc`: the USB product string.
const DEVPKEY_BUS_REPORTED_DEVICE_DESC: DevPropKey = DevPropKey {
    fmtid: Guid {
        data1: 0x540b_947e,
        data2: 0x8b40,
        data3: 0x45bc,
        data4: [0xa8, 0xa2, 0x6a, 0x0b, 0x89, 0x4c, 0xbd, 0xa2],
    },
    pid: 4,
};

const DIGCF_PRESENT: u32 = 0x0000_0002;
const DIGCF_DEVICEINTERFACE: u32 = 0x0000_0010;
const SPDRP_DEVICEDESC: u32 = 0x0000_0000;
const SPDRP_FRIENDLYNAME: u32 = 0x0000_000C;
const DICS_FLAG_GLOBAL: u32 = 0x0000_0001;
const DIREG_DEV: u32 = 0x0000_0001;
const KEY_READ: u32 = 0x0002_0019;
const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_BINARY: u32 = 0x0000_0008;
const ERROR_SUCCESS: i32 = 0;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ_WRITE: u32 = 0x0000_0003;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, USBPRINT_IOCTL_INDEX + 13, METHOD_BUFFERED, FILE_ANY_ACCESS)`.
const IOCTL_USBPRINT_GET_1284_ID: u32 = 0x0022_0034;

/// `HKEY_LOCAL_MACHINE`, a predefined handle value.
#[allow(clippy::cast_possible_wrap)]
const HKEY_LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;

/// Largest write handed to `WriteFile` at once; usbprint splits it anyway.
const USB_WRITE_CHUNK: usize = 16 * 1024;

type Handle = *mut c_void;
type DevInfoSet = isize;
type Hkey = isize;

const INVALID_HANDLE: isize = -1;

#[repr(C)]
struct SpDevinfoData {
    cb_size: u32,
    class_guid: Guid,
    dev_inst: u32,
    reserved: usize,
}

impl SpDevinfoData {
    fn new() -> Self {
        Self {
            cb_size: u32::try_from(std::mem::size_of::<Self>()).unwrap_or(0),
            class_guid: Guid {
                data1: 0,
                data2: 0,
                data3: 0,
                data4: [0; 8],
            },
            dev_inst: 0,
            reserved: 0,
        }
    }
}

#[repr(C)]
struct SpDeviceInterfaceData {
    cb_size: u32,
    interface_class_guid: Guid,
    flags: u32,
    reserved: usize,
}

impl SpDeviceInterfaceData {
    fn new() -> Self {
        Self {
            cb_size: u32::try_from(std::mem::size_of::<Self>()).unwrap_or(0),
            interface_class_guid: Guid {
                data1: 0,
                data2: 0,
                data3: 0,
                data4: [0; 8],
            },
            flags: 0,
            reserved: 0,
        }
    }
}

#[repr(C)]
struct DevPropKey {
    fmtid: Guid,
    pid: u32,
}

#[allow(non_snake_case)]
#[link(name = "setupapi")]
unsafe extern "system" {
    fn SetupDiGetClassDevsW(
        class_guid: *const Guid,
        enumerator: *const u16,
        parent: *mut c_void,
        flags: u32,
    ) -> DevInfoSet;
    fn SetupDiDestroyDeviceInfoList(set: DevInfoSet) -> i32;
    fn SetupDiEnumDeviceInterfaces(
        set: DevInfoSet,
        devinfo: *const SpDevinfoData,
        guid: *const Guid,
        index: u32,
        data: *mut SpDeviceInterfaceData,
    ) -> i32;
    fn SetupDiGetDeviceInterfaceDetailW(
        set: DevInfoSet,
        data: *const SpDeviceInterfaceData,
        detail: *mut u8,
        size: u32,
        required: *mut u32,
        devinfo: *mut SpDevinfoData,
    ) -> i32;
    fn SetupDiEnumDeviceInfo(set: DevInfoSet, index: u32, data: *mut SpDevinfoData) -> i32;
    fn SetupDiGetDeviceRegistryPropertyW(
        set: DevInfoSet,
        devinfo: *const SpDevinfoData,
        property: u32,
        reg_type: *mut u32,
        buffer: *mut u8,
        size: u32,
        required: *mut u32,
    ) -> i32;
    fn SetupDiGetDeviceInstanceIdW(
        set: DevInfoSet,
        devinfo: *const SpDevinfoData,
        id: *mut u16,
        size: u32,
        required: *mut u32,
    ) -> i32;
    fn SetupDiGetDevicePropertyW(
        set: DevInfoSet,
        devinfo: *const SpDevinfoData,
        key: *const DevPropKey,
        prop_type: *mut u32,
        buffer: *mut u8,
        size: u32,
        required: *mut u32,
        flags: u32,
    ) -> i32;
    fn SetupDiOpenDevRegKey(
        set: DevInfoSet,
        devinfo: *const SpDevinfoData,
        scope: u32,
        profile: u32,
        key_type: u32,
        sam: u32,
    ) -> Hkey;
    fn SetupDiOpenDeviceInterfaceRegKey(
        set: DevInfoSet,
        data: *const SpDeviceInterfaceData,
        reserved: u32,
        sam: u32,
    ) -> Hkey;
}

#[allow(non_snake_case)]
#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(
        key: Hkey,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        value_type: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
}

#[allow(non_snake_case)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn WriteFile(
        file: Handle,
        buffer: *const c_void,
        len: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn DeviceIoControl(
        device: Handle,
        code: u32,
        in_buffer: *const c_void,
        in_size: u32,
        out_buffer: *mut c_void,
        out_size: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn FlushFileBuffers(file: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

/// A USB printer reachable through `usbprint.sys`.
#[derive(Debug, Clone)]
pub struct UsbPrinterInfo {
    /// Device interface path for `CreateFileW`, e.g. `\\?\usb#vid_09c6&pid_0426#…`.
    pub path: String,
    /// Port monitor name the spooler uses for this device, e.g. `USB001`.
    pub port: Option<String>,
    /// USB product string, e.g. `BY-482BT`.
    pub product: Option<String>,
    /// IEEE 1284 device id, e.g. `MFG:PEDOOLO;MDL:BY-482BT;CMD:TSPL;`.
    pub device_id: Option<String>,
}

/// A present COM port.
#[derive(Debug, Clone)]
pub struct PortInfo {
    /// `COM5`.
    pub port: String,
    /// `Standard Serial over Bluetooth link (COM5)`.
    pub friendly: String,
    /// `PnP` instance id, e.g. `BTHENUM\{00001101-…}_…\8&…&0&E8EECC06FE6E_C00000000`.
    pub instance_id: String,
    /// Paired device name for Bluetooth links, from the Bluetooth stack.
    pub bluetooth_name: Option<String>,
}

/// Destroys a device information set when dropped.
struct DevInfoGuard(DevInfoSet);

impl Drop for DevInfoGuard {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

/// Closes a registry key when dropped.
struct KeyGuard(Hkey);

impl Drop for KeyGuard {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

/// Closes a kernel handle when dropped.
struct FileGuard(Handle);

impl Drop for FileGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decodes a UTF-16 buffer up to its first NUL.
fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Reinterprets a byte buffer filled by a `W` API as UTF-16.
fn bytes_to_wide(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect()
}

/// Enumerates USB printers bound to `usbprint.sys`.
///
/// # Errors
///
/// Returns [`Error::Io`] when `SetupAPI` cannot build the device list.
pub fn usb_printers() -> Result<Vec<UsbPrinterInfo>> {
    let set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_USBPRINT,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == INVALID_HANDLE {
        return Err(Error::io(
            "SetupDiGetClassDevs(USBPRINT)",
            std::io::Error::last_os_error(),
        ));
    }
    let guard = DevInfoGuard(set);

    let mut printers = Vec::new();
    for index in 0..256_u32 {
        let mut interface = SpDeviceInterfaceData::new();
        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(
                guard.0,
                ptr::null(),
                &GUID_DEVINTERFACE_USBPRINT,
                index,
                &raw mut interface,
            )
        };
        if ok == 0 {
            break; // ERROR_NO_MORE_ITEMS
        }

        let mut devinfo = SpDevinfoData::new();
        let Some(path) = interface_path(guard.0, &interface, &mut devinfo) else {
            continue;
        };

        let port = interface_port_name(guard.0, &interface);
        let product = device_string_property(guard.0, &devinfo, &DEVPKEY_BUS_REPORTED_DEVICE_DESC)
            .or_else(|| registry_property(guard.0, &devinfo, SPDRP_FRIENDLYNAME))
            .filter(|text| !text.trim().is_empty());
        let device_id = cached_device_id(&path);

        printers.push(UsbPrinterInfo {
            path,
            port,
            product,
            device_id,
        });
    }

    Ok(printers)
}

/// Enumerates present COM ports with their friendly names.
///
/// # Errors
///
/// Returns [`Error::Io`] when `SetupAPI` cannot build the device list.
pub fn serial_ports() -> Result<Vec<PortInfo>> {
    let set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVCLASS_PORTS,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT,
        )
    };
    if set == INVALID_HANDLE {
        return Err(Error::io(
            "SetupDiGetClassDevs(Ports)",
            std::io::Error::last_os_error(),
        ));
    }
    let guard = DevInfoGuard(set);

    let mut ports = Vec::new();
    for index in 0..1024_u32 {
        let mut devinfo = SpDevinfoData::new();
        if unsafe { SetupDiEnumDeviceInfo(guard.0, index, &raw mut devinfo) } == 0 {
            break;
        }

        let Some(port) = device_port_name(guard.0, &devinfo) else {
            continue;
        };
        // LPT ports share the class; only COM ports are byte pipes we can use.
        if !port.to_ascii_uppercase().starts_with("COM") {
            continue;
        }

        let friendly = registry_property(guard.0, &devinfo, SPDRP_FRIENDLYNAME)
            .or_else(|| registry_property(guard.0, &devinfo, SPDRP_DEVICEDESC))
            .unwrap_or_else(|| port.clone());
        let instance_id = instance_id(guard.0, &devinfo).unwrap_or_default();
        let bluetooth_name = crate::discover::bluetooth_address(&instance_id)
            .and_then(|address| bluetooth_device_name(&address));

        ports.push(PortInfo {
            port,
            friendly,
            instance_id,
            bluetooth_name,
        });
    }

    ports.sort_by_key(|port| port_number(&port.port));
    Ok(ports)
}

/// Numeric part of `COM12`, for natural sorting.
fn port_number(port: &str) -> u32 {
    port.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(u32::MAX)
}

fn interface_path(
    set: DevInfoSet,
    interface: &SpDeviceInterfaceData,
    devinfo: &mut SpDevinfoData,
) -> Option<String> {
    let mut required: u32 = 0;
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            interface,
            ptr::null_mut(),
            0,
            &raw mut required,
            ptr::null_mut(),
        );
    }
    if required < 8 {
        return None;
    }

    // SP_DEVICE_INTERFACE_DETAIL_DATA_W: a DWORD cbSize, then the path. The
    // cbSize field is the size of the fixed part, which is 8 on 64-bit and 6 on
    // 32-bit because of the trailing WCHAR's alignment.
    let mut buffer = vec![0_u8; required as usize + 2];
    let header: u32 = if cfg!(target_pointer_width = "64") {
        8
    } else {
        6
    };
    buffer[..4].copy_from_slice(&header.to_le_bytes());

    let ok = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            interface,
            buffer.as_mut_ptr(),
            required,
            ptr::null_mut(),
            devinfo,
        )
    };
    if ok == 0 {
        return None;
    }

    let path = from_wide(&bytes_to_wide(&buffer[4..]));
    (!path.is_empty()).then_some(path)
}

/// `Port Name` under the interface's `Device Parameters` key: `USB001`.
fn interface_port_name(set: DevInfoSet, interface: &SpDeviceInterfaceData) -> Option<String> {
    let key = unsafe { SetupDiOpenDeviceInterfaceRegKey(set, interface, 0, KEY_READ) };
    if key == INVALID_HANDLE || key == 0 {
        return None;
    }
    let guard = KeyGuard(key);
    registry_string(guard.0, None, "Port Name")
        .or_else(|| registry_string(guard.0, Some("Device Parameters"), "Port Name"))
}

/// `PortName` under the device's hardware key: `COM5`.
fn device_port_name(set: DevInfoSet, devinfo: &SpDevinfoData) -> Option<String> {
    let key =
        unsafe { SetupDiOpenDevRegKey(set, devinfo, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
    if key == INVALID_HANDLE || key == 0 {
        return None;
    }
    let guard = KeyGuard(key);
    registry_string(guard.0, None, "PortName")
}

fn registry_property(set: DevInfoSet, devinfo: &SpDevinfoData, property: u32) -> Option<String> {
    let mut buffer = vec![0_u8; 1024];
    let mut required: u32 = 0;
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            devinfo,
            property,
            ptr::null_mut(),
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).unwrap_or(0),
            &raw mut required,
        )
    };
    if ok == 0 {
        return None;
    }
    let text = from_wide(&bytes_to_wide(&buffer));
    (!text.is_empty()).then_some(text)
}

fn device_string_property(
    set: DevInfoSet,
    devinfo: &SpDevinfoData,
    key: &DevPropKey,
) -> Option<String> {
    let mut buffer = vec![0_u8; 1024];
    let mut prop_type: u32 = 0;
    let mut required: u32 = 0;
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            set,
            devinfo,
            key,
            &raw mut prop_type,
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).unwrap_or(0),
            &raw mut required,
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    let text = from_wide(&bytes_to_wide(&buffer));
    (!text.is_empty()).then_some(text)
}

fn instance_id(set: DevInfoSet, devinfo: &SpDevinfoData) -> Option<String> {
    let mut buffer = vec![0_u16; 512];
    let ok = unsafe {
        SetupDiGetDeviceInstanceIdW(
            set,
            devinfo,
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).unwrap_or(0),
            ptr::null_mut(),
        )
    };
    (ok != 0).then(|| from_wide(&buffer))
}

/// Reads a `REG_SZ` value from `key` (or a subkey of it).
fn registry_string(key: Hkey, subkey: Option<&str>, value: &str) -> Option<String> {
    let subkey = subkey.map(wide);
    let value = wide(value);
    let mut buffer = vec![0_u16; 260];
    let mut size = u32::try_from(buffer.len() * 2).unwrap_or(0);
    let status = unsafe {
        RegGetValueW(
            key,
            subkey.as_ref().map_or(ptr::null(), Vec::as_ptr),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let text = from_wide(&buffer);
    (!text.is_empty()).then_some(text)
}

/// Name of a paired Bluetooth device, stored by the stack as UTF-8 bytes.
fn bluetooth_device_name(address: &str) -> Option<String> {
    let subkey = wide(&format!(
        r"SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices\{}",
        address.to_ascii_lowercase()
    ));
    let value = wide("Name");
    let mut buffer = vec![0_u8; 256];
    let mut size = u32::try_from(buffer.len()).unwrap_or(0);
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    buffer.truncate(size as usize);
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    let name = String::from_utf8_lossy(&buffer[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// IEEE 1284 ids never change for a device path, so they are asked for once:
/// discovery runs every few seconds and should not keep poking the printer.
fn cached_device_id(path: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock() {
        if let Some(known) = map.get(path) {
            return known.clone();
        }
    }

    let id = query_device_id(path);
    // A busy printer can refuse the open; only remember real answers so the
    // next sweep tries again.
    if id.is_some() {
        if let Ok(mut map) = cache.lock() {
            map.insert(path.to_string(), id.clone());
        }
    }
    id
}

/// Asks usbprint for the printer's IEEE 1284 device id.
fn query_device_id(path: &str) -> Option<String> {
    let name = wide(path);
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle as isize == INVALID_HANDLE || handle.is_null() {
        return None;
    }
    let guard = FileGuard(handle);

    let mut buffer = vec![0_u8; 1024];
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            guard.0,
            IOCTL_USBPRINT_GET_1284_ID,
            ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len()).unwrap_or(0),
            &raw mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    crate::discover::decode_1284_reply(&buffer[..(returned as usize).min(buffer.len())])
}

/// Writes `job` straight to a USB printer interface.
///
/// # Errors
///
/// Returns [`Error::Io`] naming the device when it cannot be opened, a write
/// fails, or the device accepts fewer bytes than were sent.
pub fn write_device(path: &str, job: &[u8]) -> Result<()> {
    let name = wide(path);
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle as isize == INVALID_HANDLE || handle.is_null() {
        return Err(Error::io(
            format!("open USB printer {path}"),
            std::io::Error::last_os_error(),
        ));
    }
    let guard = FileGuard(handle);

    for chunk in job.chunks(USB_WRITE_CHUNK) {
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                guard.0,
                chunk.as_ptr().cast(),
                u32::try_from(chunk.len()).unwrap_or(u32::MAX),
                &raw mut written,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::io(
                format!("write USB printer {path}"),
                std::io::Error::last_os_error(),
            ));
        }
        if written as usize != chunk.len() {
            return Err(Error::io(
                format!("write USB printer {path}"),
                std::io::Error::other(format!(
                    "device accepted {written} of {} bytes",
                    chunk.len()
                )),
            ));
        }
    }

    unsafe {
        FlushFileBuffers(guard.0);
    }
    Ok(())
}
