//! Optional camera diagnostics, isolated from the USB transport and its ownership policy.
use crate::desktop::Role;
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    io,
    marker::PhantomData,
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Camera {
    pub path: String,
    pub name: String,
    pub identities: Vec<String>,
}
impl Camera {
    /// Use the complete physical USB identity, never the shared model name/VID/PID.
    pub fn stable_identity(&self) -> String {
        self.identities
            .iter()
            .find(|id| {
                let id = id.to_ascii_uppercase();
                id.starts_with("USB\\VID_") && !id.contains("&MI_")
            })
            .unwrap_or(&self.path)
            .clone()
    }
    fn preference_key(&self) -> String {
        self.stable_identity().to_ascii_lowercase()
    }

    pub fn matches(&self, identity: &str) -> bool {
        self.path.eq_ignore_ascii_case(identity)
            || self
                .identities
                .first()
                .is_some_and(|id| id.eq_ignore_ascii_case(identity))
            || self
                .identities
                .iter()
                .find(|id| {
                    id.to_ascii_uppercase().starts_with("USB\\VID_")
                        && !id.to_ascii_uppercase().contains("&MI_")
                })
                .is_some_and(|id| id.eq_ignore_ascii_case(identity))
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    #[serde(default)]
    low_light: std::collections::BTreeMap<String, bool>,
}
fn preferences_path(config: &Path) -> std::path::PathBuf {
    config.with_extension("camera.preferences.json")
}
fn preferences(config: &Path) -> io::Result<Preferences> {
    match crate::settings::read(&preferences_path(config)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Preferences::default()),
        result => result,
    }
}
pub fn preferred_low_light(config: &Path, camera: &Camera) -> io::Result<bool> {
    Ok(preferences(config)?
        .low_light
        .get(&camera.preference_key())
        .copied()
        .unwrap_or(false))
}
pub fn save_low_light(config: &Path, camera: &Camera, enabled: bool) -> io::Result<()> {
    let path = preferences_path(config);
    let _lock = crate::settings::acquire_file_lock(&path.with_extension("edit.lock"))?;
    let mut value = preferences(config)?;
    value.low_light.insert(camera.preference_key(), enabled);
    crate::settings::save(&path, &value)
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Mode {
    pub index: i32,
    pub width: i32,
    pub height: i32,
    pub interval: i64,
    pub format: String,
    pub key: String,
}
impl Mode {
    pub fn fps(&self) -> f64 {
        if self.interval > 0 {
            10_000_000.0 / self.interval as f64
        } else {
            0.0
        }
    }
    pub fn label(&self) -> String {
        format!(
            "{} × {} · {:.2} FPS · {}",
            self.width,
            self.height,
            self.fps(),
            self.format
        )
    }
}
pub fn default_mode(modes: &[Mode]) -> Option<usize> {
    modes
        .iter()
        .enumerate()
        .min_by_key(|(_, mode)| {
            (
                if mode.format == "MJPEG" { 0 } else { 1 },
                (i64::from(mode.width) - 1280).abs() + (i64::from(mode.height) - 720).abs(),
                ((mode.fps() - 30.0).abs() * 1000.0) as u64,
            )
        })
        .map(|(index, _)| index)
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[repr(C)]
pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
    pub decoded: u64,
    pub elapsed_us: u64,
    pub width: i32,
    pub height: i32,
    pub interval: i64,
    pub error: i32,
    #[serde(serialize_with = "serialize_capture_backend")]
    pub capture_backend: u32,
}
fn serialize_capture_backend<S: serde::Serializer>(
    backend: &u32,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match backend {
        1 => "windows-ksproxy",
        2 => "device-moniker",
        _ => "unknown",
    })
}
impl Stats {
    pub fn average_fps(&self) -> f64 {
        if self.elapsed_us > 0 {
            self.frames as f64 * 1e6 / self.elapsed_us as f64
        } else {
            0.0
        }
    }
}
#[derive(Clone, Copy, Default)]
pub struct Rates {
    pub received: f64,
    pub decoded: f64,
    pub mib_per_second: f64,
}
pub fn rates(before: &Stats, after: &Stats) -> Rates {
    if after.elapsed_us <= before.elapsed_us
        || after.frames < before.frames
        || after.decoded < before.decoded
        || after.bytes < before.bytes
    {
        return Rates::default();
    }
    let seconds = (after.elapsed_us - before.elapsed_us) as f64 / 1e6;
    Rates {
        received: (after.frames - before.frames) as f64 / seconds,
        decoded: (after.decoded - before.decoded) as f64 / seconds,
        mib_per_second: (after.bytes - before.bytes) as f64 / seconds / 1048576.0,
    }
}
pub struct Frame {
    /// BGRA in bottom-up DIB order, independent of the producer's orientation.
    pub pixels: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub sequence: u64,
}
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub fn fit(
    width: i32,
    height: i32,
    available_width: i32,
    available_height: i32,
) -> (i32, i32, i32, i32) {
    if width <= 0 || height <= 0 || available_width <= 0 || available_height <= 0 {
        return (0, 0, 0, 0);
    }
    let scale =
        (available_width as f64 / width as f64).min(available_height as f64 / height as f64);
    let w = ((width as f64 * scale).round() as i32).clamp(1, available_width);
    let h = ((height as f64 * scale).round() as i32).clamp(1, available_height);
    ((available_width - w) / 2, (available_height - h) / 2, w, h)
}
fn frame_bytes(width: i32, height: i32) -> Option<usize> {
    if !(1..=8192).contains(&width) || !(1..=8192).contains(&height) {
        return None;
    }
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
        .filter(|size| *size <= MAX_FRAME_BYTES)
}
unsafe extern "C" {
    fn ub_camera_thread_initialize() -> i32;
    fn ub_camera_thread_uninitialize();
    fn ub_camera_list(output: *mut *mut u16) -> i32;
    fn ub_camera_modes(path: *const u16, output: *mut *mut u16) -> i32;
    fn ub_camera_set_name(path: *const u16, name: *const u16) -> i32;
    fn ub_camera_string_free(value: *mut u16);
    fn ub_camera_open(
        path: *const u16,
        index: i32,
        key: *const u16,
        output: *mut *mut c_void,
    ) -> i32;
    fn ub_camera_poll(
        handle: *mut c_void,
        last: *mut u64,
        callback: extern "C" fn(*mut c_void, *const u8, usize, i32, i32, u64),
        context: *mut c_void,
        stats: *mut Stats,
    ) -> i32;
    fn ub_camera_controls(handle: *mut c_void, output: *mut *mut u16) -> i32;
    fn ub_camera_set_low_light(handle: *mut c_void, enabled: i32) -> i32;
    fn ub_camera_close(handle: *mut c_void) -> i32;
}
fn error(code: i32) -> io::Error {
    io::Error::other(format!("Camera / DirectShow: 0x{:08X}", code as u32))
}
fn checked(code: i32) -> io::Result<()> {
    if code < 0 { Err(error(code)) } else { Ok(()) }
}
fn json_result<T: serde::de::DeserializeOwned>(
    operation: impl FnOnce(*mut *mut u16) -> i32,
) -> io::Result<T> {
    let mut pointer = std::ptr::null_mut();
    let code = operation(&mut pointer);
    struct Text(*mut u16);
    impl Drop for Text {
        fn drop(&mut self) {
            unsafe {
                ub_camera_string_free(self.0);
            }
        }
    }
    let owned = Text(pointer);
    checked(code)?;
    if owned.0.is_null() {
        return Err(io::Error::other("Camera enumeration returned no data"));
    }
    let mut count = 0;
    // The native bridge owns a terminated UTF-16 allocation, bounded here before deserialization.
    while count < 1024 * 1024 && unsafe { *owned.0.add(count) } != 0 {
        count += 1;
    }
    if count == 1024 * 1024 {
        return Err(io::Error::other("Camera enumeration exceeds 1 MiB"));
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(owned.0, count) })
        .map_err(io::Error::other)?;
    serde_json::from_str(&value).map_err(io::Error::other)
}
pub fn list() -> io::Result<Vec<Camera>> {
    json_result(|out| unsafe { ub_camera_list(out) })
}
pub(crate) fn set_emulated_name(path: &str, name: &str) -> io::Result<()> {
    let path = crate::desktop::wide(path);
    let name = crate::desktop::wide(name);
    checked(unsafe { ub_camera_set_name(path.as_ptr(), name.as_ptr()) })
}
pub fn modes(path: &str) -> io::Result<Vec<Mode>> {
    let path = crate::desktop::wide(path);
    json_result(|out| unsafe { ub_camera_modes(path.as_ptr(), out) })
}
struct Session {
    handle: NonNull<c_void>,
    sequence: u64,
    _thread: PhantomData<Rc<()>>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Controls {
    pub supported: bool,
    pub low_light_compensation: Option<bool>,
    pub auto_exposure_priority_original: Option<i32>,
}
// Keep one MTA alive for the preview worker. Repeatedly tearing down/recreating
// the last MTA on this Windows camera path retains kernel handles per graph.
struct Apartment {
    _thread: PhantomData<Rc<()>>,
}
impl Apartment {
    fn new() -> io::Result<Self> {
        checked(unsafe { ub_camera_thread_initialize() })?;
        Ok(Self {
            _thread: PhantomData,
        })
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            ub_camera_thread_uninitialize();
        }
    }
}
impl Session {
    fn controls(&self) -> io::Result<Controls> {
        json_result(|out| unsafe { ub_camera_controls(self.handle.as_ptr(), out) })
    }
    fn set_low_light(&mut self, enabled: bool) -> io::Result<Controls> {
        checked(unsafe { ub_camera_set_low_light(self.handle.as_ptr(), i32::from(enabled)) })?;
        let controls = self.controls()?;
        crate::diagnostics::event(
            "camera.low_light.changed",
            serde_json::json!({"enabled":enabled,"controls":controls}),
        );
        Ok(controls)
    }
    fn open(camera: &Camera, mode: &Mode) -> io::Result<Self> {
        let path = crate::desktop::wide(&camera.path);
        let key = crate::desktop::wide(&mode.key);
        let mut handle = std::ptr::null_mut();
        checked(unsafe { ub_camera_open(path.as_ptr(), mode.index, key.as_ptr(), &mut handle) })?;
        if let Ok(details) =
            json_result::<serde_json::Value>(|out| unsafe { ub_camera_controls(handle, out) })
        {
            crate::diagnostics::event("camera.controls", details);
        }
        Ok(Self {
            handle: NonNull::new(handle)
                .ok_or_else(|| io::Error::other("Camera returned an empty session"))?,
            sequence: 0,
            _thread: PhantomData,
        })
    }
    fn poll(&mut self, frames: &Mutex<Option<Arc<Frame>>>) -> io::Result<Stats> {
        extern "C" fn receive(
            context: *mut c_void,
            pixels: *const u8,
            length: usize,
            width: i32,
            height: i32,
            sequence: u64,
        ) {
            let Some(expected) = frame_bytes(width, height) else {
                return;
            };
            if pixels.is_null() || length != expected {
                return;
            }
            let frames = unsafe { &*(context as *const Mutex<Option<Arc<Frame>>>) };
            let frame = Arc::new(Frame {
                pixels: unsafe { std::slice::from_raw_parts(pixels, length) }.to_vec(),
                width,
                height,
                sequence,
            });
            *frames.lock().unwrap() = Some(frame);
        }
        let mut stats = Stats::default();
        checked(unsafe {
            ub_camera_poll(
                self.handle.as_ptr(),
                &mut self.sequence,
                receive,
                (frames as *const Mutex<Option<Arc<Frame>>>)
                    .cast_mut()
                    .cast(),
                &mut stats,
            )
        })?;
        checked(stats.error)?;
        Ok(stats)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let result = unsafe { ub_camera_close(self.handle.as_ptr()) };
        crate::diagnostics::event(
            "camera.controls.restored",
            serde_json::json!({"hresult":result,"success":result >= 0}),
        );
    }
}
pub enum Action {
    List,
    Modes(String),
    Start(Camera, Mode, bool),
    LowLight(bool),
    Stop,
    Shutdown,
}
pub enum Event {
    Cameras(io::Result<Vec<Camera>>),
    Modes(String, io::Result<Vec<Mode>>),
    Started(Controls),
    LowLight(io::Result<Controls>),
    Stopped,
    Error(String),
}
pub struct Worker {
    sender: mpsc::SyncSender<Action>,
    pub events: Arc<Mutex<Vec<Event>>>,
    pub frame: Arc<Mutex<Option<Arc<Frame>>>>,
    pub stats: Arc<Mutex<Option<Stats>>>,
    finished: Arc<AtomicBool>,
}
impl Worker {
    pub fn new() -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(8);
        let events = Arc::new(Mutex::new(Vec::new()));
        let frame = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(None));
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let (queue, frames, statistics) = (events.clone(), frame.clone(), stats.clone());
        thread::Builder::new().name("camera-preview".into()).spawn(move|| {
            let apartment = match Apartment::new() {
                Ok(apartment) => apartment,
                Err(error) => {
                    queue.lock().unwrap().push(Event::Error(error.to_string()));
                    done.store(true,Ordering::Release);
                    return;
                }
            };
            let mut session:Option<Session>=None;let mut last_log=Instant::now();
            loop {
                let action=if session.is_some(){match receiver.try_recv(){Ok(action)=>Some(action),Err(mpsc::TryRecvError::Empty)=>None,Err(_)=>break}}
                    else {match receiver.recv(){Ok(action)=>Some(action),Err(_)=>break}};
                if let Some(action)=action {
                    match action {
                        Action::List=>queue.lock().unwrap().push(Event::Cameras(list())),
                        Action::Modes(path)=>{let result=modes(&path);queue.lock().unwrap().push(Event::Modes(path,result));}
                        Action::Start(camera,mode,low_light)=>{
                            *frames.lock().unwrap()=None;*statistics.lock().unwrap()=None;session.take();
                            crate::diagnostics::event("camera.start.requested",serde_json::json!({"camera":camera,"mode":mode}));
                            match Session::open(&camera,&mode) {
                                Ok(mut opened)=>{
                                    if low_light && opened.controls().is_ok_and(|c|c.supported) { let _=opened.set_low_light(true); }
                                    let controls=opened.controls().unwrap_or_default();
                                    session=Some(opened);last_log=Instant::now();queue.lock().unwrap().push(Event::Started(controls));
                                }
                                Err(error)=>{crate::diagnostics::event("camera.open.failed",serde_json::json!({"error":error.to_string()}));queue.lock().unwrap().push(Event::Error(error.to_string()));}
                            }
                        }
                        Action::LowLight(enabled)=>{
                            let result=session.as_mut().ok_or_else(||io::Error::other("Camera preview is not running")).and_then(|s|s.set_low_light(enabled));
                            queue.lock().unwrap().push(Event::LowLight(result));
                        }
                        Action::Stop=>{
                            *frames.lock().unwrap()=None;session.take();
                            crate::diagnostics::event("camera.stopped",serde_json::json!({"stats":*statistics.lock().unwrap()}));
                            queue.lock().unwrap().push(Event::Stopped);
                        }
                        Action::Shutdown=>break,
                    }
                }
                if let Some(active)=&mut session {
                    match active.poll(&frames) {
                        Ok(current)=>{
                            if current.frames > 0 && statistics.lock().unwrap().is_none_or(|previous: Stats| previous.frames == 0) {
                                crate::diagnostics::event("camera.first_frame",serde_json::json!(current));
                            }
                            *statistics.lock().unwrap()=Some(current);
                            if last_log.elapsed()>=Duration::from_secs(1){crate::diagnostics::event("camera.stats",serde_json::json!(current));last_log=Instant::now();}
                        }
                        Err(error)=>{
                            *frames.lock().unwrap()=None;
                            crate::diagnostics::event("camera.stream.failed",serde_json::json!({"error":error.to_string(),"stats":*statistics.lock().unwrap()}));
                            queue.lock().unwrap().push(Event::Error(error.to_string()));session.take();
                        }
                    }
                    thread::sleep(Duration::from_millis(15));
                }
            }
                drop(session);drop(apartment);done.store(true,Ordering::Release);
            })?;
        Ok(Self {
            sender,
            events,
            frame,
            stats,
            finished,
        })
    }
    pub fn send(&self, action: Action) -> io::Result<()> {
        self.sender
            .try_send(action)
            .map_err(|e| io::Error::other(e.to_string()))
    }
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let _ = self.sender.try_send(Action::Shutdown);
        let due = Instant::now() + timeout;
        while !self.finished.load(Ordering::Acquire) && Instant::now() < due {
            thread::sleep(Duration::from_millis(10));
        }
        self.finished.load(Ordering::Acquire)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.try_send(Action::Shutdown);
    }
}
pub fn launch(config: &Path, role: Role, identity: Option<&str>, start: bool) -> io::Result<()> {
    let mut command = Command::new(std::env::current_exe()?.with_file_name("usbbridge-agent.exe"));
    command
        .args([
            "--camera-preview",
            "--role",
            if role == Role::Server {
                "server"
            } else {
                "client"
            },
            "--config",
        ])
        .arg(config);
    if let Some(identity) = identity {
        command.args(["--camera-identity", identity]);
        if start {
            command.arg("--camera-autostart");
        }
    }
    command
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
pub fn benchmark(
    identity: &str,
    mode_index: Option<i32>,
    seconds: u64,
    low_light: bool,
    stop_file: Option<&Path>,
) -> io::Result<()> {
    if !(1..=604_800).contains(&seconds) {
        return Err(io::Error::other(
            "camera test duration must be in 1..604800 seconds",
        ));
    }
    let selected: Vec<_> = list()?
        .into_iter()
        .filter(|camera| camera.matches(identity))
        .collect();
    let [camera] = selected.as_slice() else {
        return Err(io::Error::other(
            "camera identity not found or ambiguous; use cameras to find its exact path",
        ));
    };
    let formats = modes(&camera.path)?;
    let mode = mode_index
        .and_then(|index| formats.iter().find(|m| m.index == index))
        .or_else(|| {
            if mode_index.is_none() {
                default_mode(&formats).map(|i| &formats[i])
            } else {
                None
            }
        })
        .ok_or_else(|| io::Error::other("camera mode not found"))?;
    let mut session = Session::open(camera, mode)?;
    if low_light {
        session.set_low_light(true)?;
    }
    let frames = Mutex::new(None);
    let start = Instant::now();
    let mut previous = Stats::default();
    let mut report = Instant::now();
    loop {
        let stats = session.poll(&frames)?;
        if report.elapsed() >= Duration::from_secs(1)
            || start.elapsed() >= Duration::from_secs(seconds)
        {
            let rate = rates(&previous, &stats);
            crate::diagnostics::event(
                "camera.benchmark.stats",
                serde_json::json!({"stats":stats,"received_fps":rate.received,"decoded_fps":rate.decoded}),
            );
            println!(
                "{}",
                serde_json::json!({"camera":camera.name,"mode":mode.label(),"received_fps":rate.received,"decoded_fps":rate.decoded,"average_fps":stats.average_fps(),"video_payload_mib_s":rate.mib_per_second,"stats":stats})
            );
            previous = stats;
            report = Instant::now();
            if stop_file.is_some_and(Path::exists) {
                return Ok(());
            }
        }
        if start.elapsed() >= Duration::from_secs(seconds) {
            return if stats.frames > 0 && stats.decoded > 0 {
                Ok(())
            } else {
                Err(io::Error::other("camera produced no decoded video frames"))
            };
        }
        thread::sleep(Duration::from_millis(15));
    }
}
pub fn init_preview(config: &Path, role: Role) {
    crate::i18n::set_language(crate::i18n::preferred(config));
    crate::diagnostics::init(
        config,
        if role == Role::Server {
            "server-camera"
        } else {
            "client-camera"
        },
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn low_light_survives_reopen_and_port_change_without_affecting_another_camera() {
        let dir = std::env::temp_dir().join(format!("usbbridge-camera-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("client.json");
        std::fs::write(&config, "original configuration").unwrap();
        let first = Camera {
            path: "port-a".into(),
            name: "Webcam".into(),
            identities: vec!["USB\\VID_1234&PID_5678\\SERIAL_A".into()],
        };
        let second = Camera {
            identities: vec!["USB\\VID_1234&PID_5678\\SERIAL_B".into()],
            ..first.clone()
        };
        assert!(!preferred_low_light(&config, &first).unwrap());
        save_low_light(&config, &first, true).unwrap();
        let moved = Camera {
            path: "port-b".into(),
            identities: vec!["usb\\vid_1234&pid_5678\\serial_a".into()],
            ..first.clone()
        };
        assert!(preferred_low_light(&config, &moved).unwrap());
        assert!(!preferred_low_light(&config, &second).unwrap());
        save_low_light(&config, &second, true).unwrap();
        save_low_light(&config, &moved, false).unwrap();
        assert!(!preferred_low_light(&config, &first).unwrap());
        assert!(preferred_low_light(&config, &second).unwrap());
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "original configuration"
        );
        std::fs::write(preferences_path(&config), "invalid").unwrap();
        assert!(save_low_light(&config, &first, true).is_err());
        assert_eq!(
            std::fs::read_to_string(preferences_path(&config)).unwrap(),
            "invalid"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn fps_uses_arrivals_and_elapsed_time_and_stalls_reach_zero() {
        let before = Stats {
            frames: 30,
            decoded: 29,
            bytes: 1048576,
            elapsed_us: 1_000_000,
            ..Stats::default()
        };
        let after = Stats {
            frames: 90,
            decoded: 85,
            bytes: 3145728,
            elapsed_us: 3_000_000,
            ..Stats::default()
        };
        let r = rates(&before, &after);
        assert_eq!(r.received, 30.0);
        assert_eq!(r.decoded, 28.0);
        assert_eq!(r.mib_per_second, 1.0);
        let stalled = Stats {
            elapsed_us: 4_000_000,
            ..after
        };
        assert_eq!(rates(&after, &stalled).received, 0.0);
        assert_eq!(rates(&after, &before).received, 0.0);
    }
    #[test]
    fn native_abi_and_frame_memory_are_bounded() {
        assert_eq!(std::mem::size_of::<Stats>(), 56);
        assert_eq!(frame_bytes(1280, 720), Some(3_686_400));
        for (w, h) in [(0, 720), (-1, 720), (8192, 8192), (i32::MAX, 2)] {
            assert!(frame_bytes(w, h).is_none());
        }
        assert_eq!(fit(1280, 720, 800, 800), (0, 175, 800, 450));
        assert_eq!(fit(0, 720, 800, 800), (0, 0, 0, 0));
        assert_eq!(fit(8192, 1, 1, 90), (0, 44, 1, 1));
    }
    #[test]
    fn identity_selection_never_falls_back_to_another_camera_with_the_same_name() {
        let camera = Camera {
            name: "Webcam".into(),
            path: "\\\\?\\usb#camera-a".into(),
            identities: vec!["USB\\VID_1234&PID_5678\\SERIAL_A".into()],
        };
        assert!(camera.matches("usb\\vid_1234&pid_5678\\serial_a"));
        assert!(!camera.matches("Webcam"));
        assert!(!camera.matches("USB\\VID_1234&PID_5678\\SERIAL_B"));
        let camera = Camera {
            identities: vec![
                "USB\\VID_1234&PID_5678\\SERIAL_A".into(),
                "USB\\ROOT_HUB30\\shared".into(),
                "HTREE\\ROOT\\0".into(),
            ],
            ..camera
        };
        assert!(!camera.matches("HTREE\\ROOT\\0"));
        assert!(!camera.matches("USB\\ROOT_HUB30\\shared"));
    }
}
