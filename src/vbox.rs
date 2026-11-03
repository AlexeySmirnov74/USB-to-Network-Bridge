//! Native Rust capture using the unchanged, production-signed VBoxUSB driver.
//! Public driver ABI: VirtualBox usblib-win.h; drivers remain upstream binaries.
use crate::{
    backend::{Provider, Session},
    budget::MemoryLease,
    wire::{self, Completion, Device, Submit},
};
use std::{
    collections::{HashMap, HashSet},
    io,
    mem::{size_of, zeroed},
    path::Path,
    ptr::{null, null_mut},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};
use windows_sys::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW, CM_Locate_DevNodeW,
        },
        Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, SetFileCompletionNotificationModes,
        },
        System::{
            IO::{
                CreateIoCompletionPort, DeviceIoControl, GetOverlappedResult,
                GetQueuedCompletionStatus, OVERLAPPED, PostQueuedCompletionStatus,
            },
            Threading::CreateEventW,
        },
    },
    core::GUID,
};
const INTERFACE: GUID = GUID::from_u128(0x00873fdf_cafe_80ee_aa5e_00c04fb1720b);
const fn ioctl(function: u32) -> u32 {
    0x228000 | (function << 2)
}
const SEND_URB: u32 = ioctl(0x607);
const GET_DEVICE: u32 = ioctl(0x603);
const SELECT_INTERFACE: u32 = ioctl(0x609);
const SET_CONFIG: u32 = ioctl(0x60a);
const CLAIM: u32 = ioctl(0x60b);
const CLEAR_ENDPOINT: u32 = ioctl(0x60e);
const VERSION: u32 = ioctl(0x60f);
const ABORT_ENDPOINT: u32 = ioctl(0x610);
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
struct Iso {
    length: u16,
    offset: u16,
    status: u32,
}
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
struct Urb {
    kind: u32,
    endpoint: u32,
    direction: u32,
    flags: u32,
    error: u32,
    length: u64,
    buffer: *mut u8,
    packets: u32,
    iso: [Iso; 8],
}
struct Handle(usize);
impl Handle {
    fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.raw());
        }
    }
}
fn device_path(instance: &str) -> io::Result<Vec<u16>> {
    let instance: Vec<u16> = instance.encode_utf16().chain(Some(0)).collect();
    let mut size = 0;
    let result =
        unsafe { CM_Get_Device_Interface_List_SizeW(&mut size, &INTERFACE, instance.as_ptr(), 0) };
    if result != 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("VBoxUSB interface sizing: {result}"),
        ));
    }
    let mut path = vec![0; size as usize];
    let result = unsafe {
        CM_Get_Device_Interface_ListW(&INTERFACE, instance.as_ptr(), path.as_mut_ptr(), size, 0)
    };
    if result != 0 || path.first().copied().unwrap_or(0) == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Native capture requires the selected device to be explicitly reserved with usbipd bind --force",
        ));
    }
    Ok(path)
}
fn physical_present(instance: &str) -> bool {
    let mut identity: Vec<u16> = instance.encode_utf16().chain(Some(0)).collect();
    let mut node = 0;
    unsafe { CM_Locate_DevNodeW(&mut node, identity.as_mut_ptr(), 0) == 0 }
}
fn open_device(instance: &str) -> io::Result<Handle> {
    let path = device_path(instance)?;
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            3,
            3,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(raw as usize))
}
// The low event-handle bit suppresses IOCP packets for these synchronous
// management operations. Their OVERLAPPED storage stays live until completion.
fn control(
    handle: HANDLE,
    code: u32,
    input: *const u8,
    input_len: u32,
    output: *mut u8,
    output_len: u32,
) -> io::Result<()> {
    let event = unsafe { CreateEventW(null(), 1, 0, null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    let event = Handle(event as usize);
    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    overlapped.hEvent = (event.0 | 1) as HANDLE;
    let mut bytes = 0;
    let success = unsafe {
        DeviceIoControl(
            handle,
            code,
            input.cast(),
            input_len,
            output.cast(),
            output_len,
            &mut bytes,
            &mut overlapped,
        )
    };
    if success == 0 {
        let error = unsafe { GetLastError() };
        if error != 997 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut bytes, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if bytes != output_len {
        return Err(io::Error::other(
            "unexpected VBoxUSB management response size",
        ));
    }
    Ok(())
}
fn command(handle: HANDLE, code: u32, input: &[u8]) -> io::Result<()> {
    control(
        handle,
        code,
        input.as_ptr(),
        input.len() as u32,
        null_mut(),
        0,
    )
}
fn descriptor(handle: HANDLE, index: u8, length: usize) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0; length + 8];
    buffer[..8].copy_from_slice(&[0x80, 6, index, 2, 0, 0, length as u8, (length >> 8) as u8]);
    let mut urb = Urb {
        kind: 4,
        direction: 1,
        flags: 1,
        length: buffer.len() as u64,
        buffer: buffer.as_mut_ptr(),
        ..Urb::default()
    };
    let pointer = (&mut urb as *mut Urb).cast::<u8>();
    control(
        handle,
        SEND_URB,
        pointer,
        size_of::<Urb>() as u32,
        pointer,
        size_of::<Urb>() as u32,
    )?;
    if urb.error != 0 || urb.length < 8 {
        return Err(io::Error::other("cannot read native USB configuration"));
    }
    let actual = (urb.length as usize - 8).min(length);
    buffer.drain(..8);
    buffer.truncate(actual);
    Ok(buffer)
}
pub struct NativeProvider {
    devices: Vec<Device>,
    publication: Option<Arc<crate::publication::Store>>,
    locations: std::path::PathBuf,
    debug: bool,
    idle_hid: Option<IdleHid>,
    capture: Option<Arc<crate::native_capture::Broker>>,
}
impl NativeProvider {
    pub fn load(path: &Path, debug: bool) -> io::Result<Self> {
        Ok(Self {
            devices: crate::inventory::load(path)?,
            publication: None,
            locations: path.with_file_name("device-locations.local.json"),
            debug,
            idle_hid: None,
            capture: None,
        })
    }
    pub fn with_publication(
        mut self,
        publication: Arc<crate::publication::Store>,
        capture_helper: Option<&Path>,
    ) -> io::Result<Self> {
        self.capture = capture_helper.map(|helper| {
            crate::native_capture::Broker::new(
                self.locations.parent().unwrap_or(Path::new(".")),
                helper,
            )
        });
        self.idle_hid = Some(IdleHid::start(
            publication.clone(),
            self.locations
                .with_file_name("native-hid-guards.local.json"),
            self.capture.clone(),
        )?);
        self.publication = Some(publication);
        Ok(self)
    }
}

// A published HID device needs a configured capture while waiting for IMPORT.
// The idle owner yields before a session claims its own file/IOCP, and resumes
// only after that session's file has closed. Idle reports are discarded in memory.
struct IdleHid {
    state: Arc<IdleState>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
#[derive(Default)]
struct IdleFiles {
    held: HashMap<String, IdleCapture>,
    in_use: HashSet<String>,
}
#[derive(Default)]
struct IdleState {
    files: Mutex<IdleFiles>,
    wake: Condvar,
}
struct IdleLease {
    identity: String,
    state: Arc<IdleState>,
}
impl Drop for IdleLease {
    fn drop(&mut self) {
        self.state
            .files
            .lock()
            .unwrap()
            .in_use
            .remove(&self.identity);
        self.state.wake.notify_one();
    }
}
fn hid_only(device: &Device) -> bool {
    device.class == 3
        || (device.class == 0
            && !device.interfaces.is_empty()
            && device.interfaces.iter().all(|i| i[0] == 3))
}
fn check_version(handle: &Handle) -> io::Result<()> {
    let mut version = [0; 8];
    control(handle.raw(), VERSION, null(), 0, version.as_mut_ptr(), 8)?;
    if u32::from_le_bytes(version[..4].try_into().unwrap()) != 5 {
        return Err(io::Error::other("unsupported VBoxUSB ABI version"));
    }
    Ok(())
}
fn claim_device(handle: &Handle) -> io::Result<()> {
    initialize_device(|code, buffer| {
        let pointer = buffer.as_mut_ptr();
        control(
            handle.raw(),
            code,
            pointer,
            buffer.len() as u32,
            pointer,
            buffer.len() as u32,
        )
    })
}
fn initialize_device(
    mut operation: impl FnMut(u32, &mut [u8]) -> io::Result<()>,
) -> io::Result<()> {
    // GET_DEVICE also initializes the driver's cached high-speed flag.
    // Without it, USB 2 high-speed ISO requests are scheduled as full-speed
    // requests (eight microframes become eight milliseconds).
    // VBoxUsbRt.cpp: vboxUsbRtDispatchGetDevice / vboxUsbRtUrbSend.
    operation(GET_DEVICE, &mut [0u8; size_of::<usize>()])?;
    let mut claim = [0; 2];
    operation(CLAIM, &mut claim)?;
    if claim[1] == 0 {
        return Err(io::Error::other("physical USB device is already claimed"));
    }
    Ok(())
}
struct IdleCapture {
    // Receiver closes before Session so cancellation cannot block its IOCP worker.
    completed: Receiver<Completion>,
    session: Arc<dyn Session>,
    seq: u32,
    pending: HashMap<u32, (u8, usize)>,
}
fn hid_inputs(data: &[u8]) -> io::Result<Vec<(u8, usize)>> {
    crate::usbdk::descriptors(data)?;
    let mut inputs = Vec::new();
    let mut default_alt = false;
    let mut offset = 0;
    while offset < data.len() {
        let length = data[offset] as usize;
        let item = &data[offset..offset + length];
        match item[1] {
            4 if length >= 9 => default_alt = item[3] == 0,
            5 if length >= 7 && default_alt && item[2] & 0x80 != 0 && item[3] & 3 == 3 => {
                let packet = (u16::from_le_bytes([item[4], item[5]]) & 0x7ff) as usize;
                if packet == 0
                    || packet > 1024
                    || item[2] & 15 == 0
                    || inputs.iter().any(|(ep, _)| *ep == item[2] & 15)
                {
                    return Err(wire::invalid("invalid HID interrupt endpoint"));
                }
                inputs.push((item[2] & 15, packet));
            }
            _ => {}
        }
        offset += length;
    }
    Ok(inputs)
}
impl IdleCapture {
    fn request(&mut self, endpoint: u8, length: usize, setup: [u8; 8]) -> io::Result<u32> {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.session.submit(Submit {
            seq,
            devid: 0,
            direction: if endpoint != 0 || setup[0] & 0x80 != 0 {
                1
            } else {
                0
            },
            endpoint: endpoint as u32,
            flags: 0,
            length,
            start_frame: 0,
            packet_count: 0,
            interval: 0,
            setup,
            data: Vec::new(),
            iso: Vec::new(),
        })?;
        Ok(seq)
    }
    fn exchange(&mut self, setup: [u8; 8], length: usize) -> io::Result<Vec<u8>> {
        self.request(0, length, setup)?;
        let completion = self
            .completed
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(io::Error::other)?;
        if i32::from_be_bytes(completion.header[20..24].try_into().unwrap()) != 0 {
            return Err(io::Error::other("idle HID control transfer failed"));
        }
        Ok(
            completion.data[completion.data_start..completion.data_start + completion.data_len]
                .to_vec(),
        )
    }
    fn open(device: &Device) -> io::Result<Self> {
        let provider = NativeProvider {
            devices: vec![device.clone()],
            publication: None,
            locations: std::path::PathBuf::new(),
            debug: false,
            idle_hid: None,
            capture: None,
        };
        let (sender, completed) = mpsc::sync_channel(32);
        let session = provider.open(&device.busid, sender)?;
        let mut capture = Self {
            completed,
            session,
            seq: 1,
            pending: HashMap::new(),
        };
        let mut selected = None;
        for index in 0..device.configurations {
            let header = capture.exchange([0x80, 6, index, 2, 0, 0, 9, 0], 9)?;
            if header.len() != 9 {
                return Err(wire::invalid("short idle HID configuration"));
            }
            if device.configuration == 0 || header[5] == device.configuration {
                selected = Some((index, header[5], u16::from_le_bytes([header[2], header[3]])));
                break;
            }
        }
        let (index, configuration, length) =
            selected.ok_or_else(|| wire::invalid("missing idle HID configuration"))?;
        let data = capture.exchange(
            [0x80, 6, index, 2, 0, 0, length as u8, (length >> 8) as u8],
            length as usize,
        )?;
        let inputs = hid_inputs(&data)?;
        capture.exchange([0, 9, configuration, 0, 0, 0, 0, 0], 0)?;
        for (endpoint, length) in inputs {
            let seq = capture.request(endpoint, length, [0; 8])?;
            capture.pending.insert(seq, (endpoint, length));
        }
        Ok(capture)
    }
    fn pump(&mut self) -> io::Result<()> {
        if !self.session.connected() {
            return Err(io::ErrorKind::NotConnected.into());
        }
        // Bound work so a busy mouse cannot starve IMPORT or withdrawal.
        for _ in 0..32 {
            let completion = match self.completed.try_recv() {
                Ok(completion) => completion,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(error) => return Err(io::Error::other(error)),
            };
            let seq = u32::from_be_bytes(completion.header[4..8].try_into().unwrap());
            let status = i32::from_be_bytes(completion.header[20..24].try_into().unwrap());
            if status != 0 {
                return Err(io::Error::other(format!(
                    "idle HID interrupt status {status}"
                )));
            }
            let (endpoint, length) = self
                .pending
                .remove(&seq)
                .ok_or_else(|| wire::invalid("unknown idle HID completion"))?;
            // Payload is discarded; it is never interpreted, forwarded or logged.
            drop(completion);
            let seq = self.request(endpoint, length, [0; 8])?;
            self.pending.insert(seq, (endpoint, length));
        }
        Ok(())
    }
}
impl IdleHid {
    fn start(
        publication: Arc<crate::publication::Store>,
        status: std::path::PathBuf,
        capture: Option<Arc<crate::native_capture::Broker>>,
    ) -> io::Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let state = Arc::new(IdleState::default());
        // Acknowledgement is also used by Publish-Device to wait for the idle
        // file to close before Windows restores the selected local HID driver.
        crate::settings::save(
            &status,
            &serde_json::json!({"process_id":std::process::id(),"revision":"","held":[]}),
        )?;
        let (copy, stop) = (state.clone(), stopping.clone());
        let worker = thread::Builder::new()
            .name("native-hid-idle".into())
            .spawn(move || Self::run(publication, status, copy, stop, capture))?;
        Ok(Self {
            state,
            stopping,
            worker: Some(worker),
        })
    }
    fn lease(&self, identity: &str) -> io::Result<IdleLease> {
        let identity = identity.to_ascii_uppercase();
        let mut files = self.state.files.lock().unwrap();
        if !files.in_use.insert(identity.clone()) {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "HID session already active",
            ));
        }
        drop(files.held.remove(&identity));
        crate::diagnostics::event(
            "native.hid.idle.yielded",
            serde_json::json!({"identity":identity}),
        );
        Ok(IdleLease {
            identity,
            state: self.state.clone(),
        })
    }
    fn run(
        publication: Arc<crate::publication::Store>,
        status: std::path::PathBuf,
        state: Arc<IdleState>,
        stop: Arc<AtomicBool>,
        capture: Option<Arc<crate::native_capture::Broker>>,
    ) {
        let mut errors = HashMap::new();
        let mut acknowledged = HashSet::new();
        let mut acknowledged_revision = String::new();
        while !stop.load(Ordering::Acquire) {
            let snapshot = publication.snapshot();
            let selected: HashMap<_, _> = snapshot
                .devices
                .iter()
                .filter(|d| {
                    !snapshot
                        .local_when_idle
                        .iter()
                        .any(|id| id.eq_ignore_ascii_case(&d.path))
                })
                .filter(|d| hid_only(d))
                .map(|d| (d.path.to_ascii_uppercase(), d.clone()))
                .collect();
            let mut files = state.files.lock().unwrap();
            files.held.retain(|identity, capture| {
                let keep = selected.contains_key(identity)
                    && device_path(identity).is_ok()
                    && capture.pump().is_ok();
                if !keep {
                    crate::diagnostics::event(
                        "native.hid.idle.released",
                        serde_json::json!({"identity":identity}),
                    );
                }
                keep
            });
            errors.retain(|identity, _| selected.contains_key(identity));
            for (identity, device) in selected {
                if files.held.contains_key(&identity) || files.in_use.contains(&identity) {
                    continue;
                }
                match IdleCapture::open(&device) {
                    Ok(handle) => {
                        errors.remove(&identity);
                        crate::diagnostics::event(
                            "native.hid.idle.opened",
                            serde_json::json!({"identity":identity,"claimed":true,"configuration":device.configuration,"idle_polling":true,"payload_logging":false}),
                        );
                        files.held.insert(identity, handle);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if errors.get(&identity) != Some(&message) {
                            crate::diagnostics::event(
                                "native.hid.idle.failed",
                                serde_json::json!({"identity":identity,"error":message}),
                            );
                        }
                        errors.insert(identity, message);
                    }
                }
            }
            let current: HashSet<_> = files.held.keys().cloned().collect();
            if current != acknowledged || snapshot.revision != acknowledged_revision {
                if let Err(error) = crate::settings::save(
                    &status,
                    &serde_json::json!({"process_id":std::process::id(),"revision":snapshot.revision,"held":files.held.keys().collect::<Vec<_>>()}),
                ) {
                    files.held.clear();
                    crate::diagnostics::event(
                        "native.hid.idle.status.failed",
                        serde_json::json!({"error":error.to_string()}),
                    );
                } else {
                    acknowledged = current;
                    acknowledged_revision = snapshot.revision;
                }
            }
            drop(
                state
                    .wake
                    .wait_timeout(files, std::time::Duration::from_millis(250))
                    .unwrap(),
            );
            if let Some(capture) = &capture {
                capture.drain();
                for device in snapshot.devices.iter().filter(|d| {
                    snapshot
                        .local_when_idle
                        .iter()
                        .any(|id| id.eq_ignore_ascii_case(&d.path))
                }) {
                    let files = state.files.lock().unwrap();
                    if !files.in_use.contains(&device.path.to_ascii_uppercase())
                        && device_path(&device.path).is_ok()
                    {
                        capture.restore_idle(&device.path);
                    }
                }
            }
        }
        state.files.lock().unwrap().held.clear();
        let _ = crate::settings::save(
            &status,
            &serde_json::json!({"process_id":std::process::id(),"held":[]}),
        );
    }
}
impl Drop for IdleHid {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.state.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Provider for NativeProvider {
    fn devices(&self) -> io::Result<Vec<Device>> {
        #[derive(serde::Deserialize)]
        struct Location {
            identity: String,
            busid: String,
        }
        let locations: Vec<Location> = crate::settings::read(&self.locations).unwrap_or_default();
        let snapshot = self.publication.as_ref().map(|p| p.snapshot());
        let devices = snapshot
            .as_ref()
            .map(|p| p.devices.clone())
            .unwrap_or_else(|| self.devices.clone());
        let local_when_idle = snapshot.map(|s| s.local_when_idle).unwrap_or_default();
        Ok(devices
            .into_iter()
            .filter(|d| {
                if local_when_idle
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&d.path))
                {
                    physical_present(&d.path)
                } else {
                    device_path(&d.path).is_ok()
                }
            })
            .map(|mut d| {
                if let Some(location) = locations
                    .iter()
                    .find(|l| l.identity.eq_ignore_ascii_case(&d.path))
                    && let Some((bus, port)) = location
                        .busid
                        .split_once('-')
                        .and_then(|(b, p)| Some((b.parse::<u32>().ok()?, p.parse::<u32>().ok()?)))
                {
                    d.busid = location.busid.clone();
                    d.bus = bus;
                    d.address = port;
                }
                d
            })
            .collect())
    }
    fn open(&self, busid: &str, completed: SyncSender<Completion>) -> io::Result<Arc<dyn Session>> {
        let device = self
            .devices()?
            .into_iter()
            .find(|d| d.busid == busid)
            .ok_or(io::ErrorKind::NotFound)?;
        let local = self.publication.as_ref().is_some_and(|p| {
            p.snapshot()
                .local_when_idle
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&device.path))
        });
        let idle_lease = if hid_only(&device) || local {
            self.idle_hid
                .as_ref()
                .map(|idle| idle.lease(&device.path))
                .transpose()?
        } else {
            None
        };
        let capture_lease = if local {
            Some(
                self.capture
                    .as_ref()
                    .ok_or_else(|| {
                        io::Error::other("Native on-demand capture helper is not configured")
                    })?
                    .acquire(&device.path)?,
            )
        } else {
            None
        };
        let handle = open_device(&device.path)?;
        check_version(&handle)?;
        claim_device(&handle)?;
        let mut configurations = HashMap::new();
        for index in 0..device.configurations {
            let header = descriptor(handle.raw(), index, 9)?;
            if header.len() != 9 {
                return Err(wire::invalid("short configuration header"));
            }
            let length = u16::from_le_bytes([header[2], header[3]]) as usize;
            if length < 9 {
                return Err(wire::invalid("invalid configuration length"));
            }
            let data = descriptor(handle.raw(), index, length)?;
            let (_, endpoints) = crate::usbdk::descriptors(&data)?;
            if self.debug {
                for ep in &endpoints {
                    eprintln!(
                        "native endpoint interface={} alt={} address={:#04x} kind={} interval={} class={} subclass={} speed={}",
                        ep.interface,
                        ep.alt,
                        ep.address,
                        ep.kind,
                        ep.interval,
                        ep.class,
                        ep.subclass,
                        device.speed
                    );
                }
            }
            configurations.insert(data[5], endpoints);
        }
        let port = unsafe { CreateIoCompletionPort(handle.raw(), null_mut(), 0, 1) };
        if port.is_null() {
            return Err(io::Error::last_os_error());
        }
        let shared = Arc::new(Shared {
            skip_sync: unsafe { SetFileCompletionNotificationModes(handle.raw(), 1) } != 0,
            handle,
            port: Handle(port as usize),
            table: Mutex::new(Table::default()),
            state: Mutex::new(DeviceState {
                configuration: 0,
                alts: HashMap::new(),
            }),
            configurations,
            completed,
            memory: Arc::new(AtomicUsize::new(0)),
            stopping: AtomicBool::new(false),
            gone: AtomicBool::new(false),
            debug: self.debug,
            trace_count: AtomicUsize::new(0),
            started: Instant::now(),
        });
        let copy = shared.clone();
        let worker = thread::Builder::new()
            .name(format!("native-iocp-{busid}"))
            .spawn(move || copy.complete_loop())?;
        Ok(Arc::new(NativeSession {
            shared,
            worker: Mutex::new(Some(worker)),
            _capture_lease: capture_lease,
            _idle_lease: idle_lease,
        }))
    }
}
struct DeviceState {
    configuration: u8,
    alts: HashMap<u8, u8>,
}
struct Group {
    video: bool,
    submitted: Instant,
    first_completion_us: Option<u128>,
    request: Submit,
    buffer: Vec<u8>,
    prefix: usize,
    remaining: usize,
    actual: usize,
    status: i32,
    unlink: Option<u32>,
    memory: Option<MemoryLease>,
}
struct Pending {
    overlapped: OVERLAPPED,
    urb: Urb,
    group: Arc<Mutex<Group>>,
    iso_start: usize,
}
// Kernel-owned request storage is pinned in a Box and protected by Shared.table.
unsafe impl Send for Pending {}
#[derive(Default)]
struct Table {
    pending: HashMap<usize, Box<Pending>>,
    groups: HashMap<u32, Arc<Mutex<Group>>>,
    order: crate::ordering::ReplyOrder<Completion>,
}
struct Shared {
    handle: Handle,
    port: Handle,
    skip_sync: bool,
    table: Mutex<Table>,
    state: Mutex<DeviceState>,
    configurations: HashMap<u8, Vec<crate::usbdk::Endpoint>>,
    completed: SyncSender<Completion>,
    memory: Arc<AtomicUsize>,
    stopping: AtomicBool,
    gone: AtomicBool,
    debug: bool,
    trace_count: AtomicUsize,
    started: Instant,
}
fn usb_status(error: u32) -> i32 {
    match error {
        0 => 0,
        1 => -32,
        2 => -19,
        3 => -84,
        4 => -71,
        5 => -121,
        6 => -75,
        _ => -5,
    }
}
impl Shared {
    fn send(&self, completion: Completion) -> io::Result<()> {
        self.completed
            .send(completion)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
    fn fail(&self, r: &Submit, status: i32) -> io::Result<()> {
        // Rejected and synchronous replies can wait behind pending I/O too.
        // Account for them so an endpoint that never completes cannot turn
        // repeated rejections into an unbounded, unbudgeted reply queue.
        let memory = MemoryLease::reserve(
            &self.memory,
            128 + r.iso.len() * size_of::<wire::IsoPacket>(),
            128 * 1024 * 1024,
        )
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::OutOfMemory, "native reply budget exhausted")
        })?;
        if self.debug && status != 0 {
            eprintln!(
                "native reject seq={} ep={} status={status}",
                r.seq, r.endpoint
            );
        }
        let iso = r
            .iso
            .iter()
            .map(|p| wire::IsoPacket {
                actual: 0,
                status,
                ..p.clone()
            })
            .collect();
        let mut table = self.table.lock().unwrap();
        table.order.register(endpoint_key(r), r.seq)?;
        let mut completion = Completion::submit(r, status, 0, Vec::new(), 0, iso);
        completion.memory = Some(memory);
        for reply in table.order.complete(r.seq, completion) {
            self.send(reply)?;
        }
        Ok(())
    }
    fn finish(&self, table: &mut Table, p: &Pending, error: u32) {
        let mut group = p.group.lock().unwrap();
        if group.first_completion_us.is_none() {
            group.first_completion_us = Some(group.submitted.elapsed().as_micros());
        }
        let status = if error == 0 {
            usb_status(p.urb.error)
        } else if error == 995 {
            -104
        } else {
            -5
        };
        if status == -19 || matches!(error, 6 | 55 | 433 | 1167) {
            self.gone.store(true, Ordering::Release);
        }
        if status != 0 {
            group.status = status;
        }
        let packets = p.urb.packets as usize;
        if packets == 0 {
            group.actual = if status == 0 {
                (p.urb.length as usize)
                    .saturating_sub(group.prefix)
                    .min(group.request.length)
            } else {
                0
            };
        } else {
            for i in 0..packets {
                let packet = &mut group.request.iso[p.iso_start + i];
                let result = p.urb.iso[i];
                packet.actual = if status == 0 {
                    (result.length as u32).min(packet.length)
                } else {
                    0
                };
                packet.status = if status == 0 {
                    usb_status(result.status)
                } else {
                    status
                };
            }
        }
        group.remaining -= 1;
        if group.remaining != 0 {
            return;
        }
        table.groups.remove(&group.request.seq);
        if let Some(seq) = group.unlink {
            for reply in table
                .order
                .complete(group.request.seq, Completion::unlink(seq, -104))
            {
                let _ = self.send(reply);
            }
            return;
        }
        if !group.request.iso.is_empty() {
            if self.debug && group.video && self.trace_count.load(Ordering::Relaxed) < 4096 {
                let mut eof = 0;
                let mut errors = 0;
                let mut payloads = 0;
                for packet in &group.request.iso {
                    let start = packet.offset as usize;
                    let data = &group.buffer[start..start + packet.actual as usize];
                    if data.len() >= 2 && data[0] >= 2 && data[0] as usize <= data.len() {
                        eof += usize::from(data[1] & 2 != 0);
                        errors += usize::from(data[1] & 0x40 != 0);
                        payloads += usize::from(data.len() > data[0] as usize);
                    }
                }
                eprintln!(
                    "native video seq={} eof={} errors={} payloads={}",
                    group.request.seq, eof, errors, payloads
                );
            }
            let mut packed = 0;
            for i in 0..group.request.iso.len() {
                let packet = &group.request.iso[i];
                let start = packet.offset as usize;
                let length = packet.actual as usize;
                if group.request.direction == 1 {
                    group.buffer.copy_within(start..start + length, packed);
                }
                packed += length;
            }
            group.actual = packed;
        }
        if self.debug && self.trace_count.fetch_add(1, Ordering::Relaxed) < 4096 {
            eprintln!(
                "native complete seq={} status={} actual={} iso={} t_us={} duration_us={} first_us={} queued={} packet_errors={} empty_packets={}",
                group.request.seq,
                group.status,
                group.actual,
                group.request.iso.len(),
                self.started.elapsed().as_micros(),
                group.submitted.elapsed().as_micros(),
                group.first_completion_us.unwrap_or(0),
                table.groups.len(),
                group.request.iso.iter().filter(|p| p.status != 0).count(),
                group.request.iso.iter().filter(|p| p.actual == 0).count()
            );
        }
        // The virtual controller cannot implement remote USB wake-up.
        if group.request.endpoint == 0
            && group.request.direction == 1
            && group.request.setup[1] == 6
            && group.request.setup[3] == 2
            && group.actual >= 9
        {
            let offset = group.prefix + 7;
            group.buffer[offset] &= !0x20;
        }
        let buffer = std::mem::take(&mut group.buffer);
        let iso = std::mem::take(&mut group.request.iso);
        let mut completion = Completion::submit(
            &group.request,
            group.status,
            group.actual,
            buffer,
            group.prefix,
            iso,
        );
        completion.memory = group.memory.take();
        for reply in table.order.complete(group.request.seq, completion) {
            let _ = self.send(reply);
        }
    }
    fn complete_loop(&self) {
        loop {
            if self.stopping.load(Ordering::Acquire)
                && self.table.lock().unwrap().pending.is_empty()
            {
                return;
            }
            let (mut bytes, mut key, mut pointer) = (0, 0, null_mut());
            let success = unsafe {
                GetQueuedCompletionStatus(self.port.raw(), &mut bytes, &mut key, &mut pointer, 1000)
            };
            let error = if success == 0 {
                unsafe { GetLastError() }
            } else {
                0
            };
            if pointer.is_null() {
                continue;
            }
            let mut table = self.table.lock().unwrap();
            if let Some(p) = table.pending.remove(&(pointer as usize)) {
                self.finish(&mut table, &p, error);
            }
        }
    }
}
struct NativeSession {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    _capture_lease: Option<crate::native_capture::Lease>,
    // Field order releases the claimed session file before resuming idle capture.
    _idle_lease: Option<IdleLease>,
}
fn endpoint_key(request: &Submit) -> u8 {
    if request.endpoint == 0 {
        0
    } else {
        request.endpoint as u8 | if request.direction == 1 { 0x80 } else { 0 }
    }
}
// Split isochronous requests at the ABI's eight-packet and 16-bit offset limits.
fn iso_chunks(packets: &[wire::IsoPacket]) -> io::Result<Vec<(usize, usize, usize, usize)>> {
    let mut chunks = Vec::new();
    let mut index = 0;
    while index < packets.len() {
        let first = index;
        let base = packets[index].offset as usize;
        let mut end = base;
        while index < packets.len() && index - first < 8 {
            let packet = &packets[index];
            if packet.length > u16::MAX as u32 {
                return Err(wire::invalid("ISO packet exceeds driver ABI"));
            }
            let offset = packet.offset as usize;
            if offset < end {
                return Err(wire::invalid("overlapping ISO packets"));
            }
            if offset - base > u16::MAX as usize {
                break;
            }
            end = offset + packet.length as usize;
            index += 1;
        }
        chunks.push((first, index - first, base, end - base));
    }
    Ok(chunks)
}
impl Session for NativeSession {
    fn connected(&self) -> bool {
        !self.shared.gone.load(Ordering::Acquire)
    }
    fn submit(&self, mut request: Submit) -> io::Result<()> {
        let s = &self.shared;
        if s.debug && s.trace_count.fetch_add(1, Ordering::Relaxed) < 4096 {
            eprintln!(
                "native submit seq={} ep={} dir={} bytes={} iso={} setup={:02x?} t_us={} queued={}",
                request.seq,
                request.endpoint,
                request.direction,
                request.length,
                request.iso.len(),
                request.setup,
                s.started.elapsed().as_micros(),
                s.table.lock().unwrap().groups.len()
            );
        }
        if s.stopping.load(Ordering::Acquire) || s.gone.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let control_transfer = request.endpoint == 0;
        if control_transfer {
            let value = u16::from_le_bytes([request.setup[2], request.setup[3]]);
            let index = u16::from_le_bytes([request.setup[4], request.setup[5]]);
            let length = u16::from_le_bytes([request.setup[6], request.setup[7]]) as usize;
            if length != request.length || ((request.setup[0] >> 7) as u32) != request.direction {
                return s.fail(&request, -22);
            }
            let operation = match (request.setup[0], request.setup[1]) {
                (0, 5) => Some(Ok(())),
                (0, 9) if value <= 255 => Some(
                    command(s.handle.raw(), SET_CONFIG, &[value as u8]).map(|()| {
                        let mut state = s.state.lock().unwrap();
                        state.configuration = value as u8;
                        state.alts.clear();
                    }),
                ),
                (1, 11) if index <= 255 && value <= 255 => Some(
                    command(
                        s.handle.raw(),
                        SELECT_INTERFACE,
                        &[index as u8, value as u8],
                    )
                    .map(|()| {
                        s.state
                            .lock()
                            .unwrap()
                            .alts
                            .insert(index as u8, value as u8);
                    }),
                ),
                (2, 1) if value == 0 && index <= 255 => {
                    Some(command(s.handle.raw(), CLEAR_ENDPOINT, &[index as u8]))
                }
                _ => None,
            };
            if let Some(operation) = operation {
                return s.fail(&request, if operation.is_ok() { 0 } else { -5 });
            }
        }
        let address = request.endpoint as u8 | if request.direction == 1 { 0x80 } else { 0 };
        let (kind, video) = if control_transfer {
            (4, false)
        } else {
            let state = s.state.lock().unwrap();
            let Some(endpoint) = s.configurations.get(&state.configuration).and_then(|eps| {
                eps.iter().find(|e| {
                    e.address == address && e.alt == *state.alts.get(&e.interface).unwrap_or(&0)
                })
            }) else {
                return s.fail(&request, -32);
            };
            (
                endpoint.kind as u32,
                endpoint.class == 14 && endpoint.subclass == 2 && request.direction == 1,
            )
        };
        if (kind == 1) != (!request.iso.is_empty()) {
            return s.fail(&request, -22);
        }
        if request.flags & 0x40 != 0 && request.direction == 0 && kind == 2 {
            return s.fail(&request, -95);
        }
        let chunks = if request.iso.is_empty() {
            vec![(0, 0, 0, request.length)]
        } else {
            iso_chunks(&request.iso)?
        };
        let prefix = if control_transfer { 8 } else { 0 };
        let Some(memory) = MemoryLease::reserve(
            &s.memory,
            request.length
                + prefix
                + chunks.len() * size_of::<Pending>()
                + size_of::<Group>()
                + request.iso.len() * size_of::<wire::IsoPacket>(),
            128 * 1024 * 1024,
        ) else {
            return s.fail(&request, -12);
        };
        let mut buffer = if request.direction == 0 {
            std::mem::take(&mut request.data)
        } else {
            vec![0; request.length]
        };
        if prefix != 0 {
            buffer.resize(request.length + prefix, 0);
            buffer.copy_within(..request.length, prefix);
            buffer[..prefix].copy_from_slice(&request.setup);
        }
        let seq = request.seq;
        let group = Arc::new(Mutex::new(Group {
            video,
            submitted: Instant::now(),
            first_completion_us: None,
            request,
            buffer,
            prefix,
            remaining: chunks.len(),
            actual: 0,
            status: 0,
            unlink: None,
            memory: Some(memory),
        }));
        let mut table = s.table.lock().unwrap();
        if table.groups.contains_key(&seq) {
            return Err(wire::invalid("duplicate native request sequence"));
        }
        if table.pending.len() + chunks.len() > 4096 {
            drop(table);
            return s.fail(&group.lock().unwrap().request, -12);
        }
        table
            .order
            .register(endpoint_key(&group.lock().unwrap().request), seq)?;
        table.groups.insert(seq, group.clone());
        for (first, count, base, length) in chunks {
            let mut g = group.lock().unwrap();
            let mut urb = Urb {
                kind,
                endpoint: g.request.endpoint,
                direction: if g.request.direction == 1 { 1 } else { 2 },
                flags: if kind != 1 && g.request.flags & 1 == 0 {
                    1
                } else {
                    0
                },
                length: (length + prefix) as u64,
                buffer: unsafe { g.buffer.as_mut_ptr().add(base) },
                packets: count as u32,
                ..Urb::default()
            };
            for i in 0..count {
                let packet = &g.request.iso[first + i];
                urb.iso[i] = Iso {
                    length: packet.length as u16,
                    offset: (packet.offset as usize - base) as u16,
                    status: 0,
                };
            }
            drop(g);
            let mut pending = Box::new(Pending {
                overlapped: unsafe { zeroed() },
                urb,
                group: group.clone(),
                iso_start: first,
            });
            let pointer = &mut pending.overlapped as *mut OVERLAPPED;
            let native = (&mut pending.urb as *mut Urb).cast();
            table.pending.insert(pointer as usize, pending);
            let mut bytes = 0;
            let success = unsafe {
                DeviceIoControl(
                    s.handle.raw(),
                    SEND_URB,
                    native,
                    size_of::<Urb>() as u32,
                    native,
                    size_of::<Urb>() as u32,
                    &mut bytes,
                    pointer,
                )
            };
            if success == 0 {
                let error = unsafe { GetLastError() };
                if error != 997 {
                    let p = table.pending.remove(&(pointer as usize)).unwrap();
                    s.finish(&mut table, &p, error);
                }
            } else if s.skip_sync
                && unsafe { PostQueuedCompletionStatus(s.port.raw(), bytes, 0, pointer) } == 0
            {
                let error = unsafe { GetLastError() };
                let p = table.pending.remove(&(pointer as usize)).unwrap();
                s.finish(&mut table, &p, error);
            }
        }
        Ok(())
    }
    fn cancel(&self, seq: u32, target: u32) -> io::Result<()> {
        let s = &self.shared;
        let table = s.table.lock().unwrap();
        let Some(group) = table.groups.get(&target) else {
            drop(table);
            return s.send(Completion::unlink(seq, 0));
        };
        let mut group = group.lock().unwrap();
        if group.unlink.is_some() {
            drop(group);
            drop(table);
            return s.send(Completion::unlink(seq, 0));
        }
        group.unlink = Some(seq);
        let endpoint = group.request.endpoint as u8
            | if group.request.direction == 1 {
                0x80
            } else {
                0
            };
        drop(group);
        drop(table);
        // VBoxUSB only supports endpoint-wide aborts, not individual CancelIoEx.
        // Other in-flight transfers on this endpoint may also complete canceled.
        command(s.handle.raw(), ABORT_ENDPOINT, &[endpoint])
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        let s = &self.shared;
        s.stopping.store(true, Ordering::Release);
        let endpoints: HashSet<_> = s
            .table
            .lock()
            .unwrap()
            .groups
            .values()
            .map(|g| {
                let g = g.lock().unwrap();
                g.request.endpoint as u8 | if g.request.direction == 1 { 0x80 } else { 0 }
            })
            .collect();
        for endpoint in endpoints {
            let _ = command(s.handle.raw(), ABORT_ENDPOINT, &[endpoint]);
        }
        unsafe {
            PostQueuedCompletionStatus(s.port.raw(), 0, 0, null());
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn driver_speed_is_initialized_before_claiming_iso_device() {
        let mut speed_initialized = false;
        let mut claimed = false;
        initialize_device(|code, data| {
            match code {
                GET_DEVICE => {
                    assert!(!claimed);
                    assert_eq!(data.len(), size_of::<usize>());
                    speed_initialized = true;
                }
                CLAIM => {
                    assert!(
                        speed_initialized,
                        "ISO scheduling would use full-speed timing"
                    );
                    data[1] = 1;
                    claimed = true;
                }
                _ => panic!("unexpected initialization operation"),
            }
            Ok(())
        })
        .unwrap();
        assert!(claimed);
    }
    #[test]
    fn failed_speed_initialization_does_not_claim_or_start_transfers() {
        let mut calls = 0;
        let error = initialize_device(|code, _| {
            calls += 1;
            assert_eq!(code, GET_DEVICE);
            Err(io::Error::from_raw_os_error(1167))
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.raw_os_error(), Some(1167));
    }
    #[test]
    fn idle_hid_poll_uses_packet_size_and_ignores_outputs_and_inactive_alternates() {
        let mut config = vec![
            9, 2, 48, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 2, 3, 1, 2, 0, 7, 5, 0x81, 3, 4, 0, 10, 7,
            5, 0x02, 2, 8, 0, 10, 9, 4, 0, 1, 1, 3, 1, 2, 0, 7, 5, 0x82, 3, 8, 0, 10,
        ];
        assert_eq!(hid_inputs(&config).unwrap(), vec![(1, 4)]);
        assert!(hid_inputs(&config[..47]).is_err());
        config[22] = 0;
        assert!(hid_inputs(&config).is_err());
    }
    #[test]
    fn idle_hid_closes_completion_receiver_before_canceling_capture() {
        struct CancelOnDrop(SyncSender<Completion>);
        impl Session for CancelOnDrop {
            fn submit(&self, _: Submit) -> io::Result<()> {
                Ok(())
            }
            fn cancel(&self, _: u32, _: u32) -> io::Result<()> {
                Ok(())
            }
        }
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                assert!(matches!(
                    self.0.try_send(Completion::unlink(2, 0)),
                    Err(mpsc::TrySendError::Disconnected(_))
                ));
            }
        }
        let (sender, completed) = mpsc::sync_channel(1);
        sender.send(Completion::unlink(1, 0)).unwrap();
        drop(IdleCapture {
            completed,
            session: Arc::new(CancelOnDrop(sender)),
            seq: 3,
            pending: HashMap::new(),
        });
    }
    #[test]
    fn idle_hid_lease_excludes_other_sessions_and_releases_on_failed_open() {
        let idle = IdleHid {
            state: Arc::new(IdleState::default()),
            stopping: Arc::new(AtomicBool::new(false)),
            worker: None,
        };
        let first = idle.lease("USB\\mouse").unwrap();
        assert!(idle.lease("usb\\MOUSE").is_err());
        let other = idle.lease("USB\\other").unwrap();
        drop(first);
        assert!(idle.lease("USB\\MOUSE").is_ok());
        assert!(idle.lease("USB\\other").is_err());
        drop(other);
        assert!(idle.state.files.lock().unwrap().in_use.is_empty());
    }
    #[test]
    fn hid_guards_cover_interface_class_mice_without_touching_composite_cameras() {
        let mut device = crate::backend::mock_device();
        device.class = 0;
        device.interfaces = vec![[3, 1, 2, 0]];
        assert!(hid_only(&device));
        device.interfaces.push([14, 1, 0, 0]);
        assert!(!hid_only(&device));
        device.interfaces = vec![[14, 1, 0, 0], [1, 1, 0, 0]];
        assert!(!hid_only(&device));
        device.interfaces.clear();
        assert!(!hid_only(&device));
        device.class = 9;
        device.interfaces = vec![[3, 1, 2, 0]];
        assert!(!hid_only(&device));
    }
    #[test]
    fn abi() {
        assert_eq!(size_of::<Urb>(), 104);
        assert_eq!(size_of::<Iso>(), 8);
        assert_eq!(std::mem::offset_of!(Urb, length), 20);
        assert_eq!(std::mem::offset_of!(Urb, buffer), 28);
        assert_eq!(SEND_URB, 0x22981c);
    }
    #[test]
    fn split_iso() {
        let packets: Vec<_> = (0..17)
            .map(|i| wire::IsoPacket {
                offset: i * 1024,
                length: 1024,
                ..Default::default()
            })
            .collect();
        assert_eq!(
            iso_chunks(&packets).unwrap(),
            vec![(0, 8, 0, 8192), (8, 8, 8192, 8192), (16, 1, 16384, 1024)]
        );
        let large = vec![
            wire::IsoPacket {
                offset: 0,
                length: 50000,
                ..Default::default()
            },
            wire::IsoPacket {
                offset: 50000,
                length: 50000,
                ..Default::default()
            },
            wire::IsoPacket {
                offset: 100000,
                length: 50000,
                ..Default::default()
            },
        ];
        assert_eq!(
            iso_chunks(&large).unwrap(),
            vec![(0, 2, 0, 100000), (2, 1, 100000, 50000)]
        );
    }
}
