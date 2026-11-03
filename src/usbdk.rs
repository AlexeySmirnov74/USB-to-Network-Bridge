//! Windows capture backend with pinned request storage and overlapped IOCP I/O.
//! Native structs mirror vendor/daynix-UsbDk-*/UsbDk/UsbDkData.h (Apache-2.0).
use crate::{
    backend::{Provider, Session},
    budget::MemoryLease,
    wire::{Completion, Device, Submit},
};
use std::{
    collections::HashMap,
    ffi::c_void,
    io,
    mem::zeroed,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::SyncSender,
    },
    thread::{self, JoinHandle},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::SetFileCompletionNotificationModes,
    System::IO::{
        CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
        PostQueuedCompletionStatus,
    },
};

#[repr(C)]
#[derive(Clone)]
struct DeviceId {
    device: [u16; 200],
    instance: [u16; 200],
}
#[repr(C)]
#[derive(Clone)]
struct DeviceInfo {
    id: DeviceId,
    filter: u64,
    port: u64,
    speed: u64,
    descriptor: [u8; 18],
}
#[repr(C)]
struct ConfigRequest {
    id: DeviceId,
    index: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IsoResult {
    actual: u64,
    status: u64,
}
#[repr(C)]
#[derive(Default)]
struct TransferResult {
    bytes: u64,
    status: u64,
    iso: *mut IsoResult,
}
#[repr(C)]
#[derive(Default)]
struct TransferRequest {
    endpoint: u64,
    buffer: *mut u8,
    length: u64,
    kind: u64,
    packets: u64,
    packet_lengths: *mut u64,
    result: TransferResult,
}
type List = unsafe extern "C" fn(*mut *mut DeviceInfo, *mut u32) -> i32;
type ReleaseList = unsafe extern "C" fn(*mut DeviceInfo);
type GetConfig = unsafe extern "C" fn(*const ConfigRequest, *mut *mut u8, *mut u32) -> i32;
type ReleaseConfig = unsafe extern "C" fn(*mut u8);
type Start = unsafe extern "C" fn(*const DeviceId) -> HANDLE;
type Stop = unsafe extern "C" fn(HANDLE) -> i32;
type Pipe = unsafe extern "C" fn(HANDLE, *mut TransferRequest, *mut OVERLAPPED) -> u32;
type SystemHandle = unsafe extern "C" fn(HANDLE) -> HANDLE;
type SetAlt = unsafe extern "C" fn(HANDLE, u64, u64) -> i32;
type ResetPipe = unsafe extern "C" fn(HANDLE, u64) -> i32;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(path: *const u16, file: HANDLE, flags: u32) -> HANDLE;
    fn GetProcAddress(module: HANDLE, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: HANDLE) -> i32;
}

struct Api {
    module: usize,
    list: List,
    release_list: ReleaseList,
    get_config: GetConfig,
    release_config: ReleaseConfig,
    start: Start,
    stop: Stop,
    read: Pipe,
    write: Pipe,
    system_handle: SystemHandle,
    set_alt: SetAlt,
    reset_pipe: ResetPipe,
}
impl Drop for Api {
    fn drop(&mut self) {
        if self.module != 0 {
            unsafe {
                FreeLibrary(self.module as HANDLE);
            }
        }
    }
}
impl Api {
    fn load() -> io::Result<Arc<Self>> {
        unsafe {
            let adjacent = std::env::current_exe()?.with_file_name("UsbDkHelper.dll");
            let paths = [
                adjacent,
                PathBuf::from(
                    std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into()),
                )
                .join("UsbDk Runtime Library/UsbDkHelper.dll"),
            ];
            let path=paths.into_iter().find(|p|p.is_file()).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"UsbDkHelper.dll/capture driver missing; build or install the capture driver package first"))?;
            let path: Vec<u16> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let module = LoadLibraryExW(path.as_ptr(), null_mut(), 0x1100);
            if module.is_null() {
                return Err(io::Error::last_os_error());
            }
            macro_rules! symbol {
                ($name:literal,$ty:ty) => {{
                    let p = GetProcAddress(module, concat!($name, "\0").as_ptr());
                    if p.is_null() {
                        FreeLibrary(module);
                        return Err(io::Error::other(concat!("missing UsbDk API: ", $name)));
                    }
                    std::mem::transmute::<*mut c_void, $ty>(p)
                }};
            }
            Ok(Arc::new(Self {
                module: module as usize,
                list: symbol!("UsbDk_GetDevicesList", List),
                release_list: symbol!("UsbDk_ReleaseDevicesList", ReleaseList),
                get_config: symbol!("UsbDk_GetConfigurationDescriptor", GetConfig),
                release_config: symbol!("UsbDk_ReleaseConfigurationDescriptor", ReleaseConfig),
                start: symbol!("UsbDk_StartRedirect", Start),
                stop: symbol!("UsbDk_StopRedirect", Stop),
                read: symbol!("UsbDk_ReadPipe", Pipe),
                write: symbol!("UsbDk_WritePipe", Pipe),
                system_handle: symbol!("UsbDk_GetRedirectorSystemHandle", SystemHandle),
                set_alt: symbol!("UsbDk_SetAltsetting", SetAlt),
                reset_pipe: symbol!("UsbDk_ResetPipe", ResetPipe),
            }))
        }
    }
    fn infos(&self) -> io::Result<Vec<DeviceInfo>> {
        unsafe {
            let mut pointer = null_mut();
            let mut count = 0;
            if (self.list)(&mut pointer, &mut count) == 0 {
                return Err(io::Error::last_os_error());
            }
            let result = if count > 65535 || count > 0 && pointer.is_null() {
                Err(io::Error::other("invalid capture-driver device list"))
            } else if count == 0 {
                Ok(Vec::new())
            } else {
                Ok(std::slice::from_raw_parts(pointer, count as usize).to_vec())
            };
            (self.release_list)(pointer);
            result
        }
    }
    fn configuration(&self, info: &DeviceInfo) -> io::Result<Vec<u8>> {
        unsafe {
            let request = ConfigRequest {
                id: info.id.clone(),
                index: 0,
            };
            let mut pointer = null_mut();
            let mut length = 0;
            if (self.get_config)(&request, &mut pointer, &mut length) == 0 {
                return Err(io::Error::last_os_error());
            }
            let result = if pointer.is_null() || !(9..=65535).contains(&length) {
                Err(io::Error::other("invalid configuration descriptor"))
            } else {
                Ok(std::slice::from_raw_parts(pointer, length as usize).to_vec())
            };
            (self.release_config)(pointer);
            result
        }
    }
}
#[derive(Clone)]
pub(crate) struct Endpoint {
    pub(crate) interface: u8,
    pub(crate) alt: u8,
    pub(crate) address: u8,
    pub(crate) kind: u8,
    pub(crate) interval: u8,
    pub(crate) class: u8,
    pub(crate) subclass: u8,
}
pub(crate) fn descriptors(config: &[u8]) -> io::Result<(Vec<[u8; 4]>, Vec<Endpoint>)> {
    if config.len() < 9
        || config[1] != 2
        || u16::from_le_bytes([config[2], config[3]]) as usize != config.len()
    {
        return Err(crate::wire::invalid("invalid USB configuration"));
    }
    let mut interfaces = Vec::new();
    let mut endpoints = Vec::new();
    let (mut interface, mut alt, mut class, mut subclass) = (0, 0, 0, 0);
    let mut at = 0;
    while at < config.len() {
        let n = config[at] as usize;
        if n < 2 || at + n > config.len() {
            return Err(crate::wire::invalid("invalid USB descriptor length"));
        }
        let d = &config[at..at + n];
        match d[1] {
            4 if n >= 9 => {
                interface = d[2];
                alt = d[3];
                class = d[5];
                subclass = d[6];
                if alt == 0 {
                    interfaces.push([d[5], d[6], d[7], 0]);
                }
            }
            5 if n >= 7 => endpoints.push(Endpoint {
                interface,
                alt,
                address: d[2],
                kind: d[3] & 3,
                interval: d[6],
                class,
                subclass,
            }),
            _ => {}
        }
        at += n;
    }
    Ok((interfaces, endpoints))
}
fn busid(info: &DeviceInfo) -> String {
    format!("{}-{}", info.filter + 1, info.port + 1)
}
fn describe(info: &DeviceInfo, config: &[u8]) -> io::Result<Device> {
    if info.filter >= 65535 || info.port >= 65535 {
        return Err(crate::wire::invalid("USB bus/address outside USB/IP range"));
    }
    let d = &info.descriptor;
    let (interfaces, _) = descriptors(config)?;
    Ok(Device {
        path: format!("/usbbridge/usbdk/{}", busid(info)),
        busid: busid(info),
        bus: info.filter as u32 + 1,
        address: info.port as u32 + 1,
        // UsbDk has no Wireless-speed enum member. Its SuperSpeed=4 must
        // become Linux/USB-IP SuperSpeed=5, or USB 3 devices enumerate wrongly.
        speed: match info.speed {
            1..=3 => info.speed as u32,
            4 => 5,
            _ => return Err(crate::wire::invalid("unknown capture-driver USB speed")),
        },
        vendor: u16::from_le_bytes([d[8], d[9]]),
        product: u16::from_le_bytes([d[10], d[11]]),
        revision: u16::from_le_bytes([d[12], d[13]]),
        class: d[4],
        subclass: d[5],
        protocol: d[6],
        configuration: config[5],
        configurations: d[17],
        interfaces,
    })
}
pub struct UsbDkProvider {
    api: Arc<Api>,
}
pub fn install_capture_driver(uninstall: bool) -> io::Result<bool> {
    let root = std::env::current_exe()?.parent().unwrap().to_path_buf();
    if !uninstall && (!root.join("UsbDk.sys").is_file() || !root.join("UsbDk.inf").is_file()) {
        return Err(io::Error::other(
            "Capture package must contain UsbDk.sys and UsbDk.inf beside the server",
        ));
    }
    std::env::set_current_dir(&root)?;
    let api = Api::load()?;
    let name = if uninstall {
        c"UsbDk_UninstallDriver"
    } else {
        c"UsbDk_InstallDriver"
    };
    let pointer = unsafe { GetProcAddress(api.module as HANDLE, name.as_ptr() as *const u8) };
    if pointer.is_null() {
        return Err(io::Error::other("capture install API unavailable"));
    }
    let call: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(pointer) };
    let result = unsafe { call() };
    if uninstall {
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(false)
    } else {
        match result {
            1 => Ok(true),
            2 => Ok(false),
            _ => Err(io::Error::other(format!(
                "capture installation failed (status {result}); check administrator rights and driver signature"
            ))),
        }
    }
}
impl UsbDkProvider {
    pub fn load() -> io::Result<Self> {
        Ok(Self { api: Api::load()? })
    }
}
impl Provider for UsbDkProvider {
    fn devices(&self) -> io::Result<Vec<Device>> {
        let mut devices = Vec::new();
        for info in self.api.infos()? {
            if info.descriptor[4] == 9 {
                continue;
            }
            match self
                .api
                .configuration(&info)
                .and_then(|c| describe(&info, &c))
            {
                Ok(d) => devices.push(d),
                Err(e) => eprintln!("skip USB {}: {e}", busid(&info)),
            }
        }
        Ok(devices)
    }
    fn open(&self, id: &str, completed: SyncSender<Completion>) -> io::Result<Arc<dyn Session>> {
        let info = self
            .api
            .infos()?
            .into_iter()
            .find(|i| busid(i) == id)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let config = self.api.configuration(&info)?;
        let (_, endpoints) = descriptors(&config)?;
        let handle = unsafe { (self.api.start)(&info.id) };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let system = unsafe { (self.api.system_handle)(handle) };
        let port = unsafe { CreateIoCompletionPort(system, null_mut(), 0, 1) };
        if port.is_null() {
            unsafe {
                (self.api.stop)(handle);
            }
            return Err(io::Error::last_os_error());
        }
        // Explicitly define synchronous-completion behavior. Otherwise Windows
        // itself posts both immediate success and pending completions to IOCP.
        let skip_sync = unsafe { SetFileCompletionNotificationModes(system, 1) } != 0;
        let shared = Arc::new(Shared {
            api: self.api.clone(),
            handle: handle as usize,
            system: system as usize,
            port: port as usize,
            skip_sync,
            configuration: config[5],
            endpoints,
            alts: Mutex::new(HashMap::new()),
            pending: Mutex::new(PendingTable::default()),
            memory: Arc::new(AtomicUsize::new(0)),
            completed,
            stopping: AtomicBool::new(false),
        });
        let worker_shared = shared.clone();
        let worker = match thread::Builder::new()
            .name(format!("usb-iocp-{id}"))
            .spawn(move || worker_shared.complete_loop())
        {
            Ok(w) => w,
            Err(e) => {
                unsafe {
                    CloseHandle(port);
                    (self.api.stop)(handle);
                }
                return Err(e);
            }
        };
        Ok(Arc::new(NativeSession {
            shared,
            worker: Mutex::new(Some(worker)),
        }))
    }
}
struct Pending {
    overlapped: OVERLAPPED,
    native: TransferRequest,
    request: Submit,
    buffer: Vec<u8>,
    lengths: Vec<u64>,
    iso_results: Vec<IsoResult>,
    unlink: Option<u32>,
    memory: Option<MemoryLease>,
}
fn request_memory(length: usize, prefix: usize, packets: usize) -> Option<usize> {
    length
        .checked_add(prefix)?
        .checked_add(std::mem::size_of::<Pending>())?
        .checked_add(packets.checked_mul(
            std::mem::size_of::<crate::wire::IsoPacket>()
                + std::mem::size_of::<u64>()
                + std::mem::size_of::<IsoResult>(),
        )?)
}
// Only the submitter and IOCP worker access Pending, under Shared.pending.
// Box/Vec allocations stay pinned until the kernel reports I/O completion.
unsafe impl Send for Pending {}
#[derive(Default)]
struct PendingTable {
    transfers: HashMap<usize, Box<Pending>>,
    sequences: HashMap<u32, usize>,
}
impl PendingTable {
    fn len(&self) -> usize {
        self.transfers.len()
    }
    fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }
    fn contains_seq(&self, seq: u32) -> bool {
        self.sequences.contains_key(&seq)
    }
    fn insert(&mut self, pointer: usize, p: Box<Pending>) {
        self.sequences.insert(p.request.seq, pointer);
        self.transfers.insert(pointer, p);
    }
    fn remove(&mut self, pointer: &usize) -> Option<Box<Pending>> {
        let p = self.transfers.remove(pointer)?;
        self.sequences.remove(&p.request.seq);
        Some(p)
    }
    fn get_seq_mut(&mut self, seq: u32) -> Option<&mut Box<Pending>> {
        let pointer = *self.sequences.get(&seq)?;
        self.transfers.get_mut(&pointer)
    }
}
struct Shared {
    api: Arc<Api>,
    handle: usize,
    system: usize,
    port: usize,
    skip_sync: bool,
    configuration: u8,
    endpoints: Vec<Endpoint>,
    alts: Mutex<HashMap<u8, u8>>,
    pending: Mutex<PendingTable>,
    memory: Arc<AtomicUsize>,
    completed: SyncSender<Completion>,
    stopping: AtomicBool,
}
impl Shared {
    fn send(&self, c: Completion) -> io::Result<()> {
        self.completed
            .send(c)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
    fn fail(&self, r: &Submit, status: i32) -> io::Result<()> {
        let memory = MemoryLease::reserve(
            &self.memory,
            128 + r.iso.len() * std::mem::size_of::<crate::wire::IsoPacket>(),
            128 * 1024 * 1024,
        )
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::OutOfMemory, "capture reply budget exhausted")
        })?;
        let iso = r
            .iso
            .iter()
            .map(|p| crate::wire::IsoPacket {
                actual: 0,
                status,
                ..p.clone()
            })
            .collect();
        let mut completion = Completion::submit(r, status, 0, Vec::new(), 0, iso);
        completion.memory = Some(memory);
        self.send(completion)
    }
    fn complete_loop(&self) {
        loop {
            if self.stopping.load(Ordering::Acquire) && self.pending.lock().unwrap().is_empty() {
                break;
            }
            unsafe {
                let (mut bytes, mut key, mut pointer) = (0, 0, null_mut());
                let ok = GetQueuedCompletionStatus(
                    self.port as HANDLE,
                    &mut bytes,
                    &mut key,
                    &mut pointer,
                    1000,
                );
                let error = if ok == 0 { GetLastError() } else { 0 };
                if pointer.is_null() {
                    if self.stopping.load(Ordering::Acquire)
                        && self.pending.lock().unwrap().is_empty()
                    {
                        break;
                    }
                    continue;
                }
                // Queue the original completion before allowing a racing UNLINK
                // to report that its sequence has already completed.
                let mut pending = self.pending.lock().unwrap();
                let Some(p) = pending.remove(&(pointer as usize)) else {
                    continue;
                };
                self.finish(*p, error);
            }
        }
    }
    fn finish(&self, mut p: Pending, error: u32) {
        if let Some(seq) = p.unlink {
            let _ = self.send(Completion::unlink(seq, -104));
            return;
        }
        let status = map_status(error, p.native.result.status);
        let start = if p.request.endpoint == 0 { 8 } else { 0 };
        let mut actual = (p.native.result.bytes as usize).min(p.request.length);
        if status != 0 {
            actual = 0;
        }
        let mut iso = std::mem::take(&mut p.request.iso);
        if !iso.is_empty() {
            let mut source = 0usize;
            let mut packed = 0usize;
            for (packet, result) in iso.iter_mut().zip(p.iso_results.iter()) {
                packet.actual = (result.actual as u32).min(packet.length);
                packet.status = map_status(0, result.status);
                if p.request.direction == 1 {
                    let n = packet.actual as usize;
                    if source + n <= p.buffer.len() {
                        p.buffer.copy_within(source..source + n, packed);
                        packed += n;
                    } else {
                        packet.actual = 0;
                        packet.status = -5;
                    }
                }
                source += packet.length as usize;
            }
            if p.request.direction == 1 {
                actual = packed;
            }
        }
        let mut completion = Completion::submit(&p.request, status, actual, p.buffer, start, iso);
        completion.memory = p.memory.take();
        let _ = self.send(completion);
    }
}
fn map_status(error: u32, usb: u64) -> i32 {
    match error {
        995 => -104,
        1167 | 6 => -19,
        121 => -110,
        0 => match usb as u32 {
            0 => 0,
            0xc0000004 => -32,
            0xc0010000 | 0xc0000008 => -104,
            _ => -5,
        },
        _ => -5,
    }
}
struct NativeSession {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl Session for NativeSession {
    fn submit(&self, mut request: Submit) -> io::Result<()> {
        let s = &self.shared;
        if s.stopping.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let is_control = request.endpoint == 0;
        if is_control {
            let value = u16::from_le_bytes([request.setup[2], request.setup[3]]);
            let index = u16::from_le_bytes([request.setup[4], request.setup[5]]);
            let length = u16::from_le_bytes([request.setup[6], request.setup[7]]) as usize;
            if length != request.length || ((request.setup[0] >> 7) as u32) != request.direction {
                return s.fail(&request, -22);
            }
            let special = match (request.setup[0], request.setup[1]) {
                (0, 5) => Some(0),
                (0, 9) => Some(if value == s.configuration as u16 || value == 0 {
                    s.alts.lock().unwrap().clear();
                    0
                } else {
                    -95
                }),
                (1, 11) => Some(
                    if unsafe { (s.api.set_alt)(s.handle as HANDLE, index as u64, value as u64) }
                        != 0
                    {
                        s.alts.lock().unwrap().insert(index as u8, value as u8);
                        0
                    } else {
                        -5
                    },
                ),
                (2, 1) if value == 0 => Some(
                    if unsafe { (s.api.reset_pipe)(s.handle as HANDLE, index as u64) } != 0 {
                        0
                    } else {
                        -5
                    },
                ),
                _ => None,
            };
            if let Some(status) = special {
                return s.fail(&request, status);
            }
        }
        let endpoint = (request.endpoint as u8) | if request.direction == 1 { 0x80 } else { 0 };
        let kind =
            if is_control {
                0
            } else {
                let alts = s.alts.lock().unwrap();
                let Some(ep) = s.endpoints.iter().find(|e| {
                    e.address == endpoint && e.alt == *alts.get(&e.interface).unwrap_or(&0)
                }) else {
                    return s.fail(&request, -32);
                };
                match ep.kind {
                    1 => 3,
                    2 => 1,
                    3 => 2,
                    _ => return s.fail(&request, -95),
                }
            };
        if (kind == 3) != (!request.iso.is_empty()) {
            return s.fail(&request, -22);
        }
        // UsbDk currently does not expose USBIP URB_ZERO_PACKET semantics.
        if request.flags & 0x40 != 0 && request.direction == 0 && kind == 1 {
            return s.fail(&request, -95);
        }
        let prefix = if is_control { 8 } else { 0 };
        let bytes = request_memory(request.length, prefix, request.iso.len())
            .ok_or_else(|| crate::wire::invalid("capture request memory overflow"))?;
        let Some(memory) = MemoryLease::reserve(&s.memory, bytes, 128 * 1024 * 1024) else {
            return s.fail(&request, -12);
        };
        let mut buffer = if request.direction == 0 {
            std::mem::take(&mut request.data)
        } else {
            vec![0; request.length]
        };
        if is_control {
            buffer.resize(request.length + 8, 0);
            buffer.copy_within(..request.length, 8);
            buffer[..8].copy_from_slice(&request.setup);
        }
        let lengths = request.iso.iter().map(|p| p.length as u64).collect();
        let iso_results = vec![IsoResult::default(); request.iso.len()];
        let mut p = Box::new(Pending {
            overlapped: unsafe { zeroed() },
            native: TransferRequest::default(),
            request,
            buffer,
            lengths,
            iso_results,
            unlink: None,
            memory: Some(memory),
        });
        p.native.endpoint = endpoint as u64;
        p.native.buffer = p.buffer.as_mut_ptr();
        p.native.length = (p.request.length + prefix) as u64;
        p.native.kind = kind;
        p.native.packets = p.lengths.len() as u64;
        p.native.packet_lengths = p.lengths.as_mut_ptr();
        p.native.result.iso = p.iso_results.as_mut_ptr();
        let mut pending = s.pending.lock().unwrap();
        if pending.len() >= 4096 {
            drop(pending);
            return s.fail(&p.request, -12);
        }
        if pending.contains_seq(p.request.seq) {
            return Err(crate::wire::invalid("duplicate outstanding sequence"));
        }
        let pointer = &mut p.overlapped as *mut OVERLAPPED;
        let native = &mut p.native as *mut TransferRequest;
        let read = p.request.direction == 1;
        pending.insert(pointer as usize, p);
        let result = unsafe {
            if read {
                (s.api.read)(s.handle as HANDLE, native, pointer)
            } else {
                (s.api.write)(s.handle as HANDLE, native, pointer)
            }
        };
        if result == 0 {
            let p = pending.remove(&(pointer as usize)).unwrap();
            drop(pending);
            return s.fail(&p.request, -5);
        }
        if result == 1
            && s.skip_sync
            && unsafe { PostQueuedCompletionStatus(s.port as HANDLE, 0, 0, pointer) } == 0
        {
            let error = unsafe { GetLastError() };
            // This synchronous operation will not produce a kernel packet.
            // Retire it here if posting our synthetic packet fails.
            let p = pending.remove(&(pointer as usize)).unwrap();
            s.finish(*p, error);
        }
        Ok(())
    }
    fn cancel(&self, seq: u32, target: u32) -> io::Result<()> {
        let s = &self.shared;
        let mut pending = s.pending.lock().unwrap();
        let Some(p) = pending.get_seq_mut(target) else {
            drop(pending);
            return s.send(Completion::unlink(seq, 0));
        };
        if p.unlink.is_some() {
            drop(pending);
            return s.send(Completion::unlink(seq, 0));
        }
        p.unlink = Some(seq);
        if unsafe { CancelIoEx(s.system as HANDLE, &p.overlapped) } == 0 {
            p.unlink = None;
            drop(pending);
            return s.send(Completion::unlink(seq, 0));
        }
        Ok(())
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        let s = &self.shared;
        s.stopping.store(true, Ordering::Release);
        unsafe {
            CancelIoEx(s.system as HANDLE, null());
            PostQueuedCompletionStatus(s.port as HANDLE, 0, 0, null());
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        unsafe {
            (s.api.stop)(s.handle as HANDLE);
            CloseHandle(s.port as HANDLE);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;
    #[test]
    fn zero_payload_iso_requests_still_reserve_their_metadata_until_release() {
        let bytes = request_memory(0, 0, 16384).unwrap();
        assert!(bytes >= 16384 * 40);
        assert!(request_memory(usize::MAX, 8, 0).is_none());
        let used = Arc::new(AtomicUsize::new(0));
        let mut held = Vec::new();
        while let Some(lease) = MemoryLease::reserve(&used, bytes, 128 * 1024 * 1024) {
            held.push(lease);
        }
        assert!(held.len() < 256);
        drop(held);
        assert_eq!(used.load(Ordering::Acquire), 0);
    }
    #[test]
    fn native_layout() {
        assert_eq!(size_of::<DeviceId>(), 800);
        assert_eq!(size_of::<DeviceInfo>(), 848);
        assert_eq!(size_of::<ConfigRequest>(), 808);
        assert_eq!(size_of::<TransferRequest>(), 72);
    }
    #[test]
    fn reject_malformed_descriptors() {
        assert!(descriptors(&[9, 2, 10, 0, 1, 1, 0, 0, 0, 0]).is_err());
    }
    #[test]
    fn statuses() {
        assert_eq!(map_status(995, 0), -104);
        assert_eq!(map_status(0, 0xc0000004), -32);
        assert_eq!(map_status(0, 0), 0);
    }
    #[test]
    fn superspeed_mapping() {
        let mut info: DeviceInfo = unsafe { zeroed() };
        let config = [9, 2, 9, 0, 0, 1, 0, 0x80, 50];
        for (native, usbip) in [(1, 1), (2, 2), (3, 3), (4, 5)] {
            info.speed = native;
            assert_eq!(describe(&info, &config).unwrap().speed, usbip);
        }
        info.speed = 0;
        assert!(describe(&info, &config).is_err());
    }
    #[test]
    fn native_iocp_with_file_fixture() {
        // Exercise the real pinned-buffer/IOCP completion path without loading
        // a USB driver. Only the small USB dispatch call is replaced by ReadFile.
        use std::{
            os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
            time::Duration,
        };
        use windows_sys::Win32::{
            Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile},
            System::Pipes::{ConnectNamedPipe, CreateNamedPipeW},
        };
        unsafe extern "C" fn no_list(_: *mut *mut DeviceInfo, _: *mut u32) -> i32 {
            0
        }
        unsafe extern "C" fn no_release_list(_: *mut DeviceInfo) {}
        unsafe extern "C" fn no_config(
            _: *const ConfigRequest,
            _: *mut *mut u8,
            _: *mut u32,
        ) -> i32 {
            0
        }
        unsafe extern "C" fn no_release_config(_: *mut u8) {}
        unsafe extern "C" fn no_start(_: *const DeviceId) -> HANDLE {
            null_mut()
        }
        unsafe extern "C" fn no_stop(_: HANDLE) -> i32 {
            1
        }
        unsafe extern "C" fn identity(h: HANDLE) -> HANDLE {
            h
        }
        unsafe extern "C" fn no_alt(_: HANDLE, _: u64, _: u64) -> i32 {
            0
        }
        unsafe extern "C" fn no_reset(_: HANDLE, _: u64) -> i32 {
            0
        }
        unsafe extern "C" fn read(h: HANDLE, r: *mut TransferRequest, o: *mut OVERLAPPED) -> u32 {
            unsafe {
                (*r).result.bytes = (*r).length;
                let mut bytes = 0;
                if ReadFile(h, (*r).buffer, (*r).length as u32, &mut bytes, o) != 0 {
                    1
                } else if GetLastError() == 997 {
                    2
                } else {
                    0
                }
            }
        }
        let path = std::env::temp_dir().join(format!("usbbridge-iocp-{}.bin", std::process::id()));
        std::fs::write(&path, vec![0xa5; 65536]).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
            .unwrap();
        let handle = file.as_raw_handle();
        let port = unsafe { CreateIoCompletionPort(handle, null_mut(), 0, 1) };
        assert!(!port.is_null());
        let skip_sync = unsafe { SetFileCompletionNotificationModes(handle, 1) } != 0;
        let api = Arc::new(Api {
            module: 0,
            list: no_list,
            release_list: no_release_list,
            get_config: no_config,
            release_config: no_release_config,
            start: no_start,
            stop: no_stop,
            read,
            write: read,
            system_handle: identity,
            set_alt: no_alt,
            reset_pipe: no_reset,
        });
        let (tx, rx) = std::sync::mpsc::sync_channel(128);
        let shared = Arc::new(Shared {
            api: api.clone(),
            handle: handle as usize,
            system: handle as usize,
            port: port as usize,
            skip_sync,
            configuration: 1,
            endpoints: vec![Endpoint {
                interface: 0,
                alt: 0,
                address: 0x81,
                kind: 2,
                interval: 0,
                class: 0,
                subclass: 0,
            }],
            alts: Mutex::new(HashMap::new()),
            pending: Mutex::new(PendingTable::default()),
            memory: Arc::new(AtomicUsize::new(0)),
            completed: tx,
            stopping: AtomicBool::new(false),
        });
        let completion_shared = shared.clone();
        let worker = thread::spawn(move || completion_shared.complete_loop());
        let session = NativeSession {
            shared: shared.clone(),
            worker: Mutex::new(Some(worker)),
        };
        for seq in 0..64 {
            session
                .submit(Submit {
                    seq,
                    devid: 0x10001,
                    direction: 1,
                    endpoint: 1,
                    flags: 0,
                    length: 65536,
                    start_frame: 0,
                    packet_count: u32::MAX,
                    interval: 0,
                    setup: [0; 8],
                    data: Vec::new(),
                    iso: Vec::new(),
                })
                .unwrap();
        }
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let completion = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(crate::wire::u32be(&completion.header[20..]), 0);
            assert_eq!(completion.data_len, 65536);
            assert!(completion.data.iter().all(|&v| v == 0xa5));
            assert!(seen.insert(crate::wire::u32be(&completion.header[4..])));
        }
        assert_eq!(shared.memory.load(Ordering::Relaxed), 0);
        drop(session);
        drop(shared);
        drop(file);
        std::fs::remove_file(path).unwrap();

        // If a synchronous completion cannot be posted, no later kernel packet
        // will arrive. Its pinned request and byte lease must retire inline.
        unsafe extern "C" fn immediate(
            _: HANDLE,
            _: *mut TransferRequest,
            _: *mut OVERLAPPED,
        ) -> u32 {
            1
        }
        let fault_api = Arc::new(Api {
            module: 0,
            list: no_list,
            release_list: no_release_list,
            get_config: no_config,
            release_config: no_release_config,
            start: no_start,
            stop: no_stop,
            read: immediate,
            write: immediate,
            system_handle: identity,
            set_alt: no_alt,
            reset_pipe: no_reset,
        });
        let (tx_fault, rx_fault) = std::sync::mpsc::sync_channel(1);
        let fault = Arc::new(Shared {
            api: fault_api,
            handle: 0,
            system: 0,
            port: 0,
            skip_sync: true,
            configuration: 1,
            endpoints: vec![Endpoint {
                interface: 0,
                alt: 0,
                address: 0x81,
                kind: 2,
                interval: 0,
                class: 0,
                subclass: 0,
            }],
            alts: Mutex::new(HashMap::new()),
            pending: Mutex::new(PendingTable::default()),
            memory: Arc::new(AtomicUsize::new(0)),
            completed: tx_fault,
            stopping: AtomicBool::new(false),
        });
        let session_fault = NativeSession {
            shared: fault.clone(),
            worker: Mutex::new(None),
        };
        session_fault
            .submit(Submit {
                seq: 1,
                devid: 0x10001,
                direction: 1,
                endpoint: 1,
                flags: 0,
                length: 65536,
                start_frame: 0,
                packet_count: 0,
                interval: 0,
                setup: [0; 8],
                data: Vec::new(),
                iso: Vec::new(),
            })
            .unwrap();
        let failed = rx_fault.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(crate::wire::u32be(&failed.header[20..]), 0);
        assert!(fault.pending.lock().unwrap().is_empty());
        drop(failed);
        assert_eq!(fault.memory.load(Ordering::Acquire), 0);
        drop(session_fault);

        // An empty named pipe leaves a real overlapped read pending. Cancel it
        // through the production UNLINK/CancelIoEx path and check that only the
        // unlink response is emitted, with no stale original completion.
        let name: Vec<u16> = format!("\\\\.\\pipe\\usbbridge-cancel-{}\0", std::process::id())
            .encode_utf16()
            .collect();
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                1 | FILE_FLAG_OVERLAPPED,
                0,
                1,
                4096,
                4096,
                0,
                null(),
            )
        };
        assert_ne!(pipe, INVALID_HANDLE_VALUE);
        let writer = unsafe {
            CreateFileW(
                name.as_ptr(),
                0x40000000,
                0,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        assert_ne!(writer, INVALID_HANDLE_VALUE);
        let mut connection: OVERLAPPED = unsafe { zeroed() };
        assert_eq!(unsafe { ConnectNamedPipe(pipe, &mut connection) }, 0);
        assert_eq!(unsafe { GetLastError() }, 535); // ERROR_PIPE_CONNECTED
        let port = unsafe { CreateIoCompletionPort(pipe, null_mut(), 0, 1) };
        assert!(!port.is_null());
        let (tx, rx) = std::sync::mpsc::sync_channel(128);
        let shared = Arc::new(Shared {
            api,
            handle: pipe as usize,
            system: pipe as usize,
            port: port as usize,
            skip_sync: unsafe { SetFileCompletionNotificationModes(pipe, 1) } != 0,
            configuration: 1,
            endpoints: vec![Endpoint {
                interface: 0,
                alt: 0,
                address: 0x81,
                kind: 2,
                interval: 0,
                class: 0,
                subclass: 0,
            }],
            alts: Mutex::new(HashMap::new()),
            pending: Mutex::new(PendingTable::default()),
            memory: Arc::new(AtomicUsize::new(0)),
            completed: tx,
            stopping: AtomicBool::new(false),
        });
        let completion_shared = shared.clone();
        let session = NativeSession {
            shared: shared.clone(),
            worker: Mutex::new(Some(thread::spawn(move || {
                completion_shared.complete_loop()
            }))),
        };
        session
            .submit(Submit {
                seq: 100,
                devid: 0x10001,
                direction: 1,
                endpoint: 1,
                flags: 0,
                length: 65536,
                start_frame: 0,
                packet_count: u32::MAX,
                interval: 0,
                setup: [0; 8],
                data: Vec::new(),
                iso: Vec::new(),
            })
            .unwrap();
        assert_eq!(shared.pending.lock().unwrap().len(), 1);
        session.cancel(101, 100).unwrap();
        let completion = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(crate::wire::u32be(&completion.header), 4);
        assert_eq!(crate::wire::u32be(&completion.header[4..]), 101);
        assert_eq!(crate::wire::u32be(&completion.header[20..]) as i32, -104);
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        assert!(shared.pending.lock().unwrap().is_empty());
        assert_eq!(shared.memory.load(Ordering::Relaxed), 0);
        drop(session);
        drop(shared);
        unsafe {
            CloseHandle(writer);
            CloseHandle(pipe);
        }
    }
}
