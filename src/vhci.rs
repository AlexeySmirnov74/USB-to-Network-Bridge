//! Native ABI adapter for the production-signed USBIP-WIN2 UDE driver.
//! USB payloads go directly through WSK in the driver, never through this CLI.
use std::{
    ffi::c_void,
    io,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW,
        },
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING},
        System::IO::DeviceIoControl,
    },
    core::GUID,
};

const INTERFACE: GUID = GUID::from_u128(0xb4030c06_dc5f_4fcc_87eb_e5515a0935c0);
#[repr(C)]
struct Location {
    busid: [u8; 32],
    service: [u8; 32],
    host: [u8; 1025],
}
#[repr(C)]
struct Config {
    location: Location,
    serial: [u8; 16],
    wsk_events: u8,
}
#[repr(C)]
struct Attach {
    size: u32,
    port: i32,
    location_hash: u32,
    config: Config,
}
// The Microsoft-signed 0.9.8.1 release predates the current source layout.
// Its inherited location structure has three bytes of trailing alignment
// before serial/wsk_events, making the request 1124 rather than 1120 bytes.
#[repr(C)]
struct AttachRelease0981 {
    size: u32,
    port: i32,
    location_hash: u32,
    location: Location,
    location_padding: [u8; 3],
    serial: [u8; 16],
    wsk_events: u8,
}
#[repr(C)]
struct Detach {
    size: u32,
    port: i32,
}
struct Driver(HANDLE);
impl Drop for Driver {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn open() -> io::Result<Driver> {
    unsafe {
        let mut n = 0;
        let result = CM_Get_Device_Interface_List_SizeW(&mut n, &INTERFACE, null(), 0);
        if result != 0 {
            return Err(io::Error::other(format!(
                "CM interface sizing failed: {result}"
            )));
        }
        let mut paths = vec![0; n as usize];
        let result = CM_Get_Device_Interface_ListW(&INTERFACE, null(), paths.as_mut_ptr(), n, 0);
        if result != 0 || paths.first() == Some(&0) || paths.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "USB/IP virtual host-controller driver is not installed",
            ));
        }
        let handle = CreateFileW(
            paths.as_ptr(),
            0xc0000000,
            3,
            null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Driver(handle))
    }
}
fn copy_string(dst: &mut [u8], src: &str) -> io::Result<()> {
    crate::wire::put_string(dst, src)
}
#[repr(C)]
struct StopAttempts {
    size: u32,
    count: i32,
    location: Location,
}
#[repr(C)]
struct StopAttemptsRelease0981 {
    size: u32,
    port: i32,
    hash: u32,
    location: Location,
    padding: [u8; 3],
    count: i32,
}
pub fn stop_attach_attempts(server: &str, busid: &str) -> io::Result<()> {
    let (host, service) = crate::settings::endpoint(server)?;
    let driver = open()?;
    let mut request: StopAttempts = unsafe { zeroed() };
    request.size = size_of::<StopAttempts>() as u32;
    copy_string(&mut request.location.host, &host)?;
    copy_string(&mut request.location.service, &service.to_string())?;
    copy_string(&mut request.location.busid, busid)?;
    fn invoke<T>(driver: &Driver, request: &mut T) -> io::Result<()> {
        let mut returned = 0;
        let pointer = (request as *mut T).cast();
        if unsafe {
            DeviceIoControl(
                driver.0,
                0x22e014,
                pointer,
                size_of::<T>() as u32,
                pointer,
                size_of::<T>() as u32,
                &mut returned,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    match invoke(&driver, &mut request) {
        Err(error) if error.raw_os_error() == Some(122) => {
            let mut release = StopAttemptsRelease0981 {
                size: size_of::<StopAttemptsRelease0981>() as u32,
                port: 0,
                hash: 0,
                location: request.location,
                padding: [0; 3],
                count: 0,
            };
            invoke(&driver, &mut release)
        }
        result => result,
    }
}
pub fn attach(server: &str, busid: &str, low_latency: bool) -> io::Result<i32> {
    let (host, port) = if let Some(rest) = server.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .ok_or_else(|| io::Error::other("IPv6 requires [address]:port"))?;
        (host, port)
    } else {
        server
            .rsplit_once(':')
            .ok_or_else(|| io::Error::other("server must be HOST:PORT"))?
    };
    let port: u16 = port.parse().map_err(io::Error::other)?;
    if host.is_empty() || port == 0 {
        return Err(io::Error::other("invalid server address"));
    }
    let driver = open()?;
    let mut request: Attach = unsafe { zeroed() };
    // An empty serial override is intentional. The production driver then forwards
    // the physical iSerialNumber descriptor instead of replacing it with a synthetic ID.
    request.size = size_of::<Attach>() as u32;
    copy_string(&mut request.config.location.host, host)?;
    copy_string(&mut request.config.location.service, &port.to_string())?;
    copy_string(&mut request.config.location.busid, busid)?;
    request.config.wsk_events = low_latency as u8;
    match attach_ioctl(&driver, &mut request) {
        Ok(()) => Ok(request.port),
        Err(e) if e.raw_os_error() == Some(122) => {
            // A short-input failure is returned before import or any USB
            // state change, so retrying with the published release ABI is safe.
            let mut release = AttachRelease0981 {
                size: size_of::<AttachRelease0981>() as u32,
                port: 0,
                location_hash: 0,
                location: request.config.location,
                location_padding: [0; 3],
                serial: request.config.serial,
                wsk_events: low_latency as u8,
            };
            attach_ioctl(&driver, &mut release)?;
            Ok(release.port)
        }
        Err(e) => Err(e),
    }
}
fn attach_ioctl<T>(driver: &Driver, request: &mut T) -> io::Result<()> {
    let pointer = request as *mut T as *mut c_void;
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            driver.0,
            0x22e018,
            pointer,
            size_of::<T>() as u32,
            pointer,
            8,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let port = unsafe { *((pointer as *const u8).add(4) as *const i32) };
    if returned != 8 || port <= 0 {
        return Err(io::Error::other("invalid driver attach response"));
    }
    Ok(())
}
pub fn detach(port: i32) -> io::Result<()> {
    if port < 1 {
        return Err(io::Error::other("port must be positive"));
    }
    let driver = open()?;
    let request = Detach {
        size: size_of::<Detach>() as u32,
        port,
    };
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            driver.0,
            0x22e004,
            &request as *const Detach as *const c_void,
            request.size,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub struct ImportedDevice {
    pub port: i32,
    pub busid: String,
    pub server: String,
}
pub fn imported_devices() -> io::Result<Vec<ImportedDevice>> {
    // Both the signed 0.9.8.1 and current source ABIs have 1132-byte device
    // records. Connection identity fields share the same published offsets.
    let driver = open()?;
    let mut capacity = 4usize;
    loop {
        let size = 1136u32;
        let mut output = vec![0u8; 4 + capacity * 1132];
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                driver.0,
                0x22e008,
                (&size as *const u32).cast(),
                4,
                output.as_mut_ptr().cast(),
                output.len() as u32,
                &mut returned,
                null_mut(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(122) && capacity < 1024 {
                capacity *= 2;
                continue;
            }
            return Err(error);
        }
        let returned = returned as usize;
        if returned < 4 || returned > output.len() || !(returned - 4).is_multiple_of(1132) {
            return Err(crate::wire::invalid("invalid imported-device response"));
        }
        let mut devices = Vec::new();
        for record in output[4..returned].as_chunks::<1132>().0 {
            let port = i32::from_le_bytes(record[..4].try_into().unwrap());
            let busid = crate::wire::fixed_string(&record[8..40])?;
            let service = crate::wire::fixed_string(&record[40..72])?;
            let host = crate::wire::fixed_string(&record[72..1097])?;
            let server = if host.contains(':') {
                format!("[{host}]:{service}")
            } else {
                format!("{host}:{service}")
            };
            devices.push(ImportedDevice {
                port,
                busid,
                server,
            });
        }
        return Ok(devices);
    }
}

/// Register only this project's root device. Called only by the explicit
/// install-driver CLI command; ordinary list/attach never install drivers.
pub fn install_root(inf: &std::path::Path) -> io::Result<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        HDEVINFO, SP_DEVINFO_DATA, SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList,
        SetupDiCreateDeviceInfoW, SetupDiDestroyDeviceInfoList, SetupDiGetDeviceInstanceIdW,
        SetupDiSetDeviceRegistryPropertyW,
    };
    #[link(name = "newdev")]
    unsafe extern "system" {
        fn UpdateDriverForPlugAndPlayDevicesW(
            window: *mut c_void,
            hardware: *const u16,
            inf: *const u16,
            flags: u32,
            reboot: *mut i32,
        ) -> i32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    if unsafe { IsUserAnAdmin() } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Run driver installation as Administrator",
        ));
    }
    let mut present = 0;
    if unsafe { CM_Get_Device_Interface_List_SizeW(&mut present, &INTERFACE, null(), 0) } != 0
        || present > 1
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "A virtual USB/IP controller already exists; refusing to replace it",
        ));
    }
    let inf = inf.canonicalize()?;
    if inf.file_name().and_then(|s| s.to_str()) != Some("usbip2_ude.inf") {
        return Err(io::Error::other("Expected usbip2_ude.inf"));
    }
    let utf16 = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let class = GUID::from_u128(0x36fc9e60_c465_11cf_8056_444553540000);
    let name = utf16("USBBridgeVHCI");
    let description = utf16("USBBridge virtual USB host controller");
    let hardware = utf16("ROOT\\USBIP_WIN2\\UDE");
    let inf_text = inf.to_string_lossy();
    let inf = utf16(inf_text.strip_prefix(r"\\?\").unwrap_or(&inf_text));
    struct Install {
        set: HDEVINFO,
        data: SP_DEVINFO_DATA,
        registered: bool,
        keep: bool,
    }
    impl Drop for Install {
        fn drop(&mut self) {
            unsafe {
                if self.registered && !self.keep {
                    SetupDiCallClassInstaller(5, self.set, &self.data);
                }
                SetupDiDestroyDeviceInfoList(self.set);
            }
        }
    }
    let set = unsafe { SetupDiCreateDeviceInfoList(&class, null_mut()) };
    if set as usize == usize::MAX {
        return Err(io::Error::last_os_error());
    }
    let mut install = Install {
        set,
        data: unsafe { zeroed() },
        registered: false,
        keep: false,
    };
    install.data.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
    if unsafe {
        SetupDiCreateDeviceInfoW(
            set,
            name.as_ptr(),
            &class,
            description.as_ptr(),
            null_mut(),
            1,
            &mut install.data,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut ids = hardware.clone();
    ids.push(0);
    if unsafe {
        SetupDiSetDeviceRegistryPropertyW(
            set,
            &mut install.data,
            1,
            ids.as_ptr() as *const u8,
            (ids.len() * 2) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetupDiCallClassInstaller(0x19, set, &install.data) } == 0 {
        return Err(io::Error::last_os_error());
    }
    install.registered = true;
    let mut reboot = 0;
    if unsafe {
        UpdateDriverForPlugAndPlayDevicesW(
            null_mut(),
            hardware.as_ptr(),
            inf.as_ptr(),
            0,
            &mut reboot,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut id = vec![0u16; 512];
    if unsafe {
        SetupDiGetDeviceInstanceIdW(
            set,
            &install.data,
            id.as_mut_ptr(),
            id.len() as u32,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let n = id.iter().position(|v| *v == 0).unwrap();
    let id = String::from_utf16_lossy(&id[..n]);
    install.keep = true;
    if reboot != 0 {
        eprintln!("Windows reports that a reboot is required.");
    }
    Ok(id)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn driver_abi_sizes() {
        assert_eq!(size_of::<Location>(), 1089);
        assert_eq!(size_of::<Config>(), 1106);
        assert_eq!(size_of::<Attach>(), 1120);
        assert_eq!(size_of::<AttachRelease0981>(), 1124);
        assert_eq!(std::mem::offset_of!(AttachRelease0981, serial), 1104);
        assert_eq!(std::mem::offset_of!(AttachRelease0981, wsk_events), 1120);
        assert_eq!(size_of::<Detach>(), 8);
    }
}
