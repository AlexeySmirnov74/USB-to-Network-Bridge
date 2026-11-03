//! Camera test UI lives in its own optional process, never in the USB transport.
use super::*;
use crate::camera::{self, Action, Camera, Event, Mode, Stats};

const CAMERAS: i32 = 901;
const MODES: i32 = 902;
const RESCAN: usize = 903;
const PLAY: usize = 904;
const PRIVACY: usize = 905;
const PREVIEW_ABOUT: usize = 906;
const PREVIEW_CLOSE: usize = 907;
const LOW_LIGHT: usize = 916;
#[repr(C)]
struct CopyData {
    tag: usize,
    length: u32,
    data: *const std::ffi::c_void,
}
#[derive(Default)]
struct VideoBuffer {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    width: i32,
    height: i32,
}
impl VideoBuffer {
    fn prepare(&mut self, target: HDC, width: i32, height: i32) -> Option<HDC> {
        if width <= 0
            || height <= 0
            || i64::from(width) * i64::from(height) * 4 > camera::MAX_FRAME_BYTES as i64
        {
            return None;
        }
        unsafe {
            if self.dc.is_null() {
                self.dc = CreateCompatibleDC(target);
                if self.dc.is_null() {
                    return None;
                }
            }
            if self.width != width || self.height != height {
                let bitmap = CreateCompatibleBitmap(target, width, height);
                if bitmap.is_null() {
                    return None;
                }
                let previous = SelectObject(self.dc, bitmap);
                if previous.is_null() || previous as isize == -1 {
                    DeleteObject(bitmap);
                    return None;
                }
                if self.previous.is_null() {
                    self.previous = previous;
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                self.bitmap = bitmap;
                self.width = width;
                self.height = height;
            }
        }
        Some(self.dc)
    }
}
impl Drop for VideoBuffer {
    fn drop(&mut self) {
        unsafe {
            if !self.dc.is_null() {
                if !self.previous.is_null() {
                    SelectObject(self.dc, self.previous);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                DeleteDC(self.dc);
            }
        }
    }
}
struct Context {
    low_light: Cell<HWND>,
    low_light_supported: Cell<Option<bool>>,
    low_light_confirmed: Cell<bool>,
    config: PathBuf,
    role: Role,
    window: Cell<HWND>,
    camera: Cell<HWND>,
    mode: Cell<HWND>,
    video: Cell<HWND>,
    video_buffer: RefCell<VideoBuffer>,
    camera_label: Cell<HWND>,
    mode_label: Cell<HWND>,
    refresh: Cell<HWND>,
    play: Cell<HWND>,
    privacy: Cell<HWND>,
    fps: Cell<HWND>,
    summary: Cell<HWND>,
    status: Cell<HWND>,
    fonts: RefCell<theme::Fonts>,
    worker: camera::Worker,
    devices: RefCell<Vec<Camera>>,
    modes: RefCell<Vec<Mode>>,
    requested: RefCell<Option<String>>,
    busy: Cell<bool>,
    running: Cell<bool>,
    painted: Cell<u64>,
    painted_sequence: Cell<u64>,
    last_stats: RefCell<Stats>,
    last_update: Cell<std::time::Instant>,
    last_frame: Cell<std::time::Instant>,
    last_frames: Cell<u64>,
    status_key: RefCell<String>,
    status_error: RefCell<String>,
    pending_selection: Cell<bool>,
    auto_start: Cell<bool>,
    startup_retry: Cell<bool>,
    retry_pending: Cell<bool>,
}
fn title(role: Role) -> String {
    format!(
        "{} — {} — {} v.{}",
        crate::APP_NAME,
        role.label(),
        crate::i18n::t("Camera test"),
        crate::APP_VERSION
    )
}
fn class(role: Role) -> &'static str {
    if role == Role::Server {
        "USBBridge.Camera.Server.2"
    } else {
        "USBBridge.Camera.Client.2"
    }
}
fn notify_parent_modal(role: Role, preview: HWND) {
    unsafe {
        let class = if role == Role::Server {
            "USBBridge.Server.2"
        } else {
            "USBBridge.Client.2"
        };
        let parent = FindWindowW(wide(class).as_ptr(), null());
        if !parent.is_null() {
            PostMessageW(parent, CAMERA_MODAL, preview as usize, 0);
        }
    }
}
impl Context {
    fn save_low_light(&self, enabled: bool) {
        if self.role != Role::Client {
            return;
        }
        if let Some(camera) = self.selected_camera() {
            match camera::save_low_light(&self.config, &camera, enabled) {
                Ok(()) => {
                    crate::diagnostics::event(
                        "camera.preference.saved",
                        serde_json::json!({"camera":camera.path, "low_light":enabled}),
                    );
                    self.text(if self.running.get() {
                        "Camera setting applied and saved"
                    } else {
                        "Camera setting saved"
                    });
                }
                Err(error) => {
                    *self.status_key.borrow_mut() = "Camera setting could not be saved".into();
                    *self.status_error.borrow_mut() = error.to_string();
                    self.status_text();
                    crate::diagnostics::event(
                        "camera.preference.error",
                        serde_json::json!({"error":error.to_string()}),
                    );
                }
            }
        }
    }
    fn text(&self, key: &str) {
        *self.status_key.borrow_mut() = key.into();
        self.status_error.borrow_mut().clear();
        self.status_text();
    }
    fn status_text(&self) {
        let key = self.status_key.borrow();
        let error = self.status_error.borrow();
        set_text(
            self.status.get(),
            &if error.is_empty() {
                crate::i18n::t(&key).to_string()
            } else {
                format!("{}\r\n{}", crate::i18n::t(&key), error)
            },
        );
    }
    fn fail(&self, error: impl ToString) {
        self.busy.set(false);
        self.running.set(false);
        if !self.pending_selection.get() {
            self.auto_start.set(false);
        }
        *self.status_key.borrow_mut()="Camera could not be opened. It may be busy, disconnected, or blocked by Windows camera permissions.".into();
        *self.status_error.borrow_mut() = error.to_string();
        self.status_text();
        self.enabled();
        unsafe {
            InvalidateRect(self.video.get(), null(), 1);
        }
    }
    fn selected_camera(&self) -> Option<Camera> {
        let index = unsafe { SendMessageW(self.camera.get(), CB_GETCURSEL, 0, 0) };
        usize::try_from(index)
            .ok()
            .and_then(|i| self.devices.borrow().get(i).cloned())
    }
    fn selected_mode(&self) -> Option<Mode> {
        let index = unsafe { SendMessageW(self.mode.get(), CB_GETCURSEL, 0, 0) };
        usize::try_from(index)
            .ok()
            .and_then(|i| self.modes.borrow().get(i).cloned())
    }
    fn enabled(&self) {
        unsafe {
            let idle = !self.busy.get() && !self.running.get();
            EnableWindow(
                self.camera.get(),
                i32::from(idle && !self.devices.borrow().is_empty()),
            );
            EnableWindow(
                self.mode.get(),
                i32::from(idle && !self.modes.borrow().is_empty()),
            );
            EnableWindow(self.refresh.get(), i32::from(idle));
            EnableWindow(
                self.low_light.get(),
                i32::from(
                    !self.busy.get()
                        && self.selected_mode().is_some()
                        && self
                            .low_light_supported
                            .get()
                            .unwrap_or(!self.running.get()),
                ),
            );
            EnableWindow(
                self.play.get(),
                i32::from(
                    !self.busy.get() && (self.running.get() || self.selected_mode().is_some()),
                ),
            );
        }
        set_text(
            self.play.get(),
            crate::i18n::t(if self.busy.get() {
                "Please wait…"
            } else if self.running.get() {
                "Stop preview"
            } else {
                "Start preview"
            }),
        );
    }
    fn labels(&self) {
        set_text(self.window.get(), &title(self.role));
        set_text(self.camera_label.get(), crate::i18n::t("Camera"));
        set_text(self.mode_label.get(), crate::i18n::t("Video mode"));
        set_text(self.refresh.get(), crate::i18n::t("Refresh cameras"));
        set_text(self.privacy.get(), crate::i18n::t("Camera permissions"));
        set_text(
            self.low_light.get(),
            crate::i18n::t(if self.role == Role::Client {
                "Low-light compensation (may reduce FPS; saved per camera)"
            } else {
                "Low-light compensation (may reduce FPS)"
            }),
        );
        unsafe {
            let bar = CreateMenu();
            let file = CreatePopupMenu();
            let help = CreatePopupMenu();
            AppendMenuW(
                file,
                MF_STRING,
                PREVIEW_CLOSE,
                wide(crate::i18n::t("Закрыть")).as_ptr(),
            );
            AppendMenuW(
                help,
                MF_STRING,
                PREVIEW_ABOUT,
                wide(crate::i18n::t("About")).as_ptr(),
            );
            AppendMenuW(
                bar,
                MF_POPUP,
                file as usize,
                wide(crate::i18n::t("Файл")).as_ptr(),
            );
            AppendMenuW(
                bar,
                MF_POPUP,
                help as usize,
                wide(crate::i18n::t("Справка")).as_ptr(),
            );
            append_language_menu(bar);
            let old = GetMenu(self.window.get());
            SetMenu(self.window.get(), bar);
            theme::destroy_menu(old);
            theme::menu(bar, true);
            DrawMenuBar(self.window.get());
        }
        self.enabled();
        self.status_text();
    }
    fn layout(&self) {
        if self.camera.get().is_null() {
            return;
        }
        unsafe {
            let window = self.window.get();
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(window, &mut rect);
            let s = |value| scale(window, value);
            let width = rect.right;
            let bottom = rect.bottom;
            let field = width - s(224);
            for (label, input, y) in [
                (self.camera_label.get(), self.camera.get(), 20),
                (self.mode_label.get(), self.mode.get(), 94),
            ] {
                MoveWindow(
                    label,
                    s(20),
                    s(y),
                    if y == 20 { width - s(285) } else { field },
                    s(22),
                    1,
                );
                MoveWindow(input, s(20), s(y + 27), field, s(280), 1);
                SendMessageW(input, CB_SETITEMHEIGHT, usize::MAX, s(28) as isize);
                SendMessageW(input, CB_SETITEMHEIGHT, 0, s(28) as isize);
            }
            MoveWindow(self.refresh.get(), width - s(190), s(47), s(170), s(34), 1);
            MoveWindow(self.play.get(), width - s(190), s(121), s(170), s(34), 1);
            MoveWindow(self.low_light.get(), s(20), s(168), width - s(40), s(28), 1);
            MoveWindow(
                self.video.get(),
                s(20),
                s(208),
                width - s(40),
                (bottom - s(392)).max(s(90)),
                1,
            );
            MoveWindow(
                self.fps.get(),
                s(20),
                bottom - s(170),
                width - s(40),
                s(34),
                1,
            );
            MoveWindow(
                self.summary.get(),
                s(20),
                bottom - s(127),
                width - s(40),
                s(44),
                1,
            );
            MoveWindow(
                self.status.get(),
                s(20),
                bottom - s(72),
                width - s(40),
                s(56),
                1,
            );
            MoveWindow(self.privacy.get(), width - s(245), s(10), s(225), s(34), 1);
            theme::redraw(window, false);
        }
    }
    fn load_modes(&self) {
        if self.busy.get() || self.running.get() {
            return;
        }
        self.modes.borrow_mut().clear();
        self.low_light_supported.set(None);
        unsafe {
            SendMessageW(self.mode.get(), CB_RESETCONTENT, 0, 0);
        }
        if let Some(camera) = self.selected_camera() {
            let low_light = if self.role == Role::Client {
                match camera::preferred_low_light(&self.config, &camera) {
                    Ok(value) => value,
                    Err(error) => {
                        crate::diagnostics::event(
                            "camera.preference.error",
                            serde_json::json!({"error":error.to_string()}),
                        );
                        false
                    }
                }
            } else {
                false
            };
            unsafe {
                SendMessageW(self.low_light.get(), BM_SETCHECK, usize::from(low_light), 0);
            }
            crate::diagnostics::event("camera.ui.select", serde_json::json!({"camera":camera}));
            *self.requested.borrow_mut() = Some(camera.path.clone());
            self.busy.set(true);
            self.text("Reading video modes…");
            self.enabled();
            if let Err(error) = self.worker.send(Action::Modes(camera.path)) {
                self.fail(error);
            }
        } else {
            self.enabled();
        }
    }
    fn command(&self, id: usize) {
        if let Some(language) = language_choice(id) {
            if let Err(error) = crate::tray::change_language(&self.config, self.role, language) {
                information(self.window.get(), &error.to_string());
            }
            return;
        }
        crate::diagnostics::event(
            "camera.ui.action",
            serde_json::json!({"id":id,"camera":self.selected_camera().as_ref().map(|c|&c.path),"mode":self.selected_mode().as_ref().map(|m|&m.key)}),
        );
        match id {
            PREVIEW_ABOUT => about(self.window.get()),
            PREVIEW_CLOSE => unsafe {
                PostMessageW(self.window.get(), WM_CLOSE, 0, 0);
            },
            RESCAN if !self.busy.get() && !self.running.get() => {
                self.busy.set(true);
                self.text("Finding cameras…");
                self.enabled();
                if let Err(error) = self.worker.send(Action::List) {
                    self.fail(error);
                }
            }
            PLAY if !self.busy.get() => {
                if self.running.get() {
                    self.busy.set(true);
                    self.text("Stopping preview…");
                    self.enabled();
                    if let Err(error) = self.worker.send(Action::Stop) {
                        self.fail(error);
                    }
                } else if let (Some(camera), Some(mode)) =
                    (self.selected_camera(), self.selected_mode())
                {
                    let retry = self.retry_pending.replace(false);
                    self.startup_retry.set(!retry);
                    self.busy.set(true);
                    *self.last_stats.borrow_mut() = Stats::default();
                    self.painted.set(0);
                    self.painted_sequence.set(0);
                    self.last_frames.set(0);
                    self.last_frame.set(std::time::Instant::now());
                    set_text(self.fps.get(), "FPS: —");
                    set_text(self.summary.get(), "");
                    self.text(if retry {
                        "No frames received. Retrying this camera once…"
                    } else {
                        "Opening camera…"
                    });
                    self.enabled();
                    let low_light =
                        unsafe { SendMessageW(self.low_light.get(), BM_GETCHECK, 0, 0) }
                            == BST_CHECKED as isize;
                    if let Err(error) = self.worker.send(Action::Start(camera, mode, low_light)) {
                        self.fail(error);
                    }
                }
            }
            LOW_LIGHT if !self.busy.get() => {
                let enabled = unsafe { SendMessageW(self.low_light.get(), BM_GETCHECK, 0, 0) }
                    == BST_CHECKED as isize;
                crate::diagnostics::event(
                    "camera.ui.low_light",
                    serde_json::json!({"enabled":enabled,"running":self.running.get()}),
                );
                if self.running.get() {
                    self.busy.set(true);
                    self.enabled();
                    if let Err(error) = self.worker.send(Action::LowLight(enabled)) {
                        self.busy.set(false);
                        *self.status_error.borrow_mut() = error.to_string();
                        self.status_text();
                        self.enabled();
                    }
                } else {
                    self.save_low_light(enabled);
                }
            }
            PRIVACY => unsafe {
                ShellExecuteW(
                    self.window.get(),
                    wide("open").as_ptr(),
                    wide("ms-settings:privacy-webcam").as_ptr(),
                    null(),
                    null(),
                    SW_SHOWNORMAL,
                );
            },
            _ => {}
        }
    }
    fn tick(&self) {
        for event in std::mem::take(&mut *self.worker.events.lock().unwrap()) {
            match event {
                Event::Cameras(Ok(mut cameras)) => {
                    cameras.sort_by_key(|camera| {
                        (camera.name.to_lowercase(), camera.path.to_lowercase())
                    });
                    crate::diagnostics::event(
                        "camera.enumerated",
                        serde_json::json!({"cameras":cameras}),
                    );
                    self.busy.set(false);
                    self.modes.borrow_mut().clear();
                    unsafe {
                        SendMessageW(self.camera.get(), CB_RESETCONTENT, 0, 0);
                        SendMessageW(self.mode.get(), CB_RESETCONTENT, 0, 0);
                        for (index, camera) in cameras.iter().enumerate() {
                            let serial = camera
                                .identities
                                .iter()
                                .find(|id| id.starts_with("USB\\VID_") && !id.contains("&MI_"))
                                .and_then(|id| id.rsplit('\\').next());
                            let label = format!(
                                "{} · {}",
                                camera.name,
                                serial
                                    .map(str::to_string)
                                    .unwrap_or_else(|| (index + 1).to_string())
                            );
                            SendMessageW(
                                self.camera.get(),
                                CB_ADDSTRING,
                                0,
                                wide(&label).as_ptr() as isize,
                            );
                        }
                    }
                    let selected = match self.requested.borrow().as_deref() {
                        Some(identity) => {
                            let matches: Vec<_> = cameras
                                .iter()
                                .enumerate()
                                .filter(|(_, c)| c.matches(identity))
                                .map(|(i, _)| i)
                                .collect();
                            if matches.len() == 1 {
                                Some(matches[0])
                            } else {
                                None
                            }
                        }
                        None => (!cameras.is_empty()).then_some(0),
                    };
                    *self.devices.borrow_mut() = cameras;
                    unsafe {
                        SendMessageW(
                            self.camera.get(),
                            CB_SETCURSEL,
                            selected.unwrap_or(usize::MAX),
                            0,
                        );
                    }
                    if selected.is_some() {
                        self.load_modes();
                    } else if self.devices.borrow().is_empty() {
                        self.auto_start.set(false);
                        self.text("No camera is available in Windows. On the server, stop sharing the camera before a local test. On the client, connect it first.");
                    } else {
                        self.auto_start.set(false);
                        self.text("The selected USB camera is not available here. Connect it, or explicitly choose another camera.");
                    }
                    self.enabled();
                }
                Event::Modes(path, Ok(mut modes)) => {
                    if self
                        .selected_camera()
                        .is_none_or(|camera| camera.path != path)
                    {
                        continue;
                    }
                    modes.sort_by_key(|m| (m.width, m.height, m.format.clone(), m.interval));
                    modes.dedup_by(|a, b| a.key == b.key);
                    let selected = camera::default_mode(&modes);
                    crate::diagnostics::event(
                        "camera.modes",
                        serde_json::json!({"path":path,"modes":modes,"default":selected}),
                    );
                    unsafe {
                        SendMessageW(self.mode.get(), CB_RESETCONTENT, 0, 0);
                        for mode in &modes {
                            SendMessageW(
                                self.mode.get(),
                                CB_ADDSTRING,
                                0,
                                wide(&mode.label()).as_ptr() as isize,
                            );
                        }
                        SendMessageW(
                            self.mode.get(),
                            CB_SETCURSEL,
                            selected.unwrap_or(usize::MAX),
                            0,
                        );
                    }
                    *self.modes.borrow_mut() = modes;
                    self.busy.set(false);
                    self.text(if selected.is_some() {
                        "Ready. Choose a mode and press Start preview."
                    } else {
                        "The camera did not report a supported video mode."
                    });
                    self.enabled();
                    if !self.pending_selection.get()
                        && self.auto_start.replace(false)
                        && selected.is_some()
                    {
                        self.command(PLAY);
                    }
                }
                Event::Started(controls) => {
                    self.low_light_supported.set(Some(controls.supported));
                    self.low_light_confirmed
                        .set(controls.low_light_compensation.unwrap_or(false));
                    unsafe {
                        SendMessageW(
                            self.low_light.get(),
                            BM_SETCHECK,
                            usize::from(self.low_light_confirmed.get()),
                            0,
                        );
                    }
                    self.busy.set(false);
                    self.running.set(true);
                    self.last_frame.set(std::time::Instant::now());
                    self.text(
                        "Preview running. FPS counts received camera frames, not window repaints.",
                    );
                    self.enabled();
                }
                Event::LowLight(result) => {
                    self.busy.set(false);
                    match result {
                        Ok(controls) => {
                            self.low_light_supported.set(Some(controls.supported));
                            self.low_light_confirmed
                                .set(controls.low_light_compensation.unwrap_or(false));
                            self.text("Camera setting applied");
                            if controls.supported {
                                self.save_low_light(self.low_light_confirmed.get());
                            }
                        }
                        Err(error) => {
                            *self.status_key.borrow_mut() =
                                "Camera setting could not be changed".into();
                            *self.status_error.borrow_mut() = error.to_string();
                            self.status_text();
                        }
                    }
                    unsafe {
                        SendMessageW(
                            self.low_light.get(),
                            BM_SETCHECK,
                            usize::from(self.low_light_confirmed.get()),
                            0,
                        );
                    }
                    self.enabled();
                }
                Event::Stopped => {
                    self.busy.set(false);
                    self.running.set(false);
                    self.text("Preview stopped. The USB connection remains available.");
                    self.enabled();
                    if self.retry_pending.get() && !self.pending_selection.get() {
                        self.command(PLAY);
                    }
                }
                Event::Error(error) => self.fail(error),
                Event::Cameras(Err(error)) => self.fail(error),
                Event::Modes(_, Err(error)) => self.fail(error),
            }
        }
        if self.pending_selection.get() && !self.busy.get() {
            if self.running.get() {
                self.command(PLAY);
            } else {
                self.pending_selection.set(false);
                self.command(RESCAN);
            }
        }
        let sequence = self
            .worker
            .frame
            .lock()
            .unwrap()
            .as_ref()
            .map(|frame| frame.sequence)
            .unwrap_or(0);
        if sequence != self.painted_sequence.get() && unsafe { IsIconic(self.window.get()) == 0 } {
            unsafe {
                InvalidateRect(self.video.get(), null(), 0);
                UpdateWindow(self.video.get());
            }
        }
        if self.last_update.get().elapsed() >= Duration::from_secs(1) {
            self.last_update.set(std::time::Instant::now());
            let language = crate::i18n::preferred(&self.config);
            if language != crate::i18n::language() {
                crate::i18n::set_language(language);
                self.labels();
            }
            if let Some(stats) = *self.worker.stats.lock().unwrap() {
                if stats.decoded != self.last_frames.get() {
                    self.last_frame.set(std::time::Instant::now());
                    self.last_frames.set(stats.decoded);
                }
                let rate = camera::rates(&self.last_stats.borrow(), &stats);
                set_text(
                    self.fps.get(),
                    &crate::i18n::format(
                        "Received: {fps} FPS  ·  Decoded: {decoded} FPS",
                        &[
                            ("fps", format!("{:.2}", rate.received)),
                            ("decoded", format!("{:.2}", rate.decoded)),
                        ],
                    ),
                );
                set_text(
                    self.summary.get(),
                    &crate::i18n::format(
                        "{width} × {height}  ·  Frames: {frames}  ·  Average: {average} FPS  ·  Video: {speed} MiB/s",
                        &[
                            ("width", stats.width.to_string()),
                            ("height", stats.height.to_string()),
                            ("frames", stats.frames.to_string()),
                            ("average", format!("{:.2}", stats.average_fps())),
                            ("speed", format!("{:.2}", rate.mib_per_second)),
                        ],
                    ),
                );
                *self.last_stats.borrow_mut() = stats;
            }
            if self.running.get() && self.last_frame.get().elapsed() > Duration::from_secs(3) {
                self.text("No new frames. Check the camera or connection; you can stop and retry the preview.");
            } else if self.running.get()
                && self.status_key.borrow().as_str()
                    == "No new frames. Check the camera or connection; you can stop and retry the preview."
            {
                self.text(
                    "Preview running. FPS counts received camera frames, not window repaints.",
                );
            }
            if self.running.get()
                && !self.busy.get()
                && self.last_frame.get().elapsed() > Duration::from_secs(5)
                && self.last_stats.borrow().frames == 0
                && self.startup_retry.replace(false)
                && !self.pending_selection.get()
            {
                crate::diagnostics::event(
                    "camera.startup.retry",
                    serde_json::json!({"camera":self.selected_camera(),"mode":self.selected_mode(),"stats":*self.last_stats.borrow()}),
                );
                self.retry_pending.set(true);
                self.command(PLAY);
            }
        }
    }
}
unsafe extern "system" fn video_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    let context = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Context;
    if message == WM_ERASEBKGND {
        return 1;
    }
    if message == WM_PAINT && !context.is_null() {
        let context = unsafe { &*context };
        unsafe {
            let mut paint: PAINTSTRUCT = std::mem::zeroed();
            let target = BeginPaint(window, &mut paint);
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(window, &mut rect);
            let mut buffer = context.video_buffer.borrow_mut();
            let Some(dc) = buffer.prepare(target, rect.right, rect.bottom) else {
                EndPaint(window, &paint);
                return 0;
            };
            FillRect(dc, &rect, GetStockObject(BLACK_BRUSH) as HBRUSH);
            let frame = context.worker.frame.lock().unwrap().clone();
            let mut sequence = None;
            if let Some(frame) = frame {
                let (x, y, width, height) =
                    camera::fit(frame.width, frame.height, rect.right, rect.bottom);
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: frame.width,
                        biHeight: frame.height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    },
                    ..std::mem::zeroed()
                };
                SetStretchBltMode(dc, COLORONCOLOR);
                let result = StretchDIBits(
                    dc,
                    x,
                    y,
                    width,
                    height,
                    0,
                    0,
                    frame.width,
                    frame.height,
                    frame.pixels.as_ptr().cast(),
                    &info,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
                if result != 0 && result != -1 {
                    sequence = Some(frame.sequence);
                }
            } else {
                context.painted_sequence.set(0);
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(dc, theme::MUTED);
                SelectObject(dc, context.fonts.borrow().regular);
                let text = wide(crate::i18n::t("Choose a camera and press Start preview"));
                DrawTextW(
                    dc,
                    text.as_ptr(),
                    -1,
                    &mut rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                );
            }
            // Compose the background and image off screen, then present once. Never expose
            // the black clearing pass between frames, including during resize/uncover.
            if BitBlt(target, 0, 0, rect.right, rect.bottom, dc, 0, 0, SRCCOPY) != 0
                && let Some(sequence) = sequence
                && sequence != context.painted_sequence.get()
            {
                context.painted.set(context.painted.get() + 1);
                context.painted_sequence.set(sequence);
            }
            EndPaint(window, &paint);
        }
        return 0;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Context;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let context = unsafe { &*pointer };
    if let Some(result) = theme::message(window, message, wparam, lparam) {
        return result;
    }
    match message {
        WM_COPYDATA => {
            if lparam == 0 {
                return 0;
            }
            let data = unsafe { &*(lparam as *const CopyData) };
            if !matches!(data.tag, 1 | 2)
                || data.length < 4
                || data.length > 8192
                || !data.length.is_multiple_of(2)
                || data.data.is_null()
            {
                return 0;
            }
            let text = unsafe {
                std::slice::from_raw_parts(data.data.cast::<u16>(), data.length as usize / 2)
            };
            let Some((&0, text)) = text.split_last() else {
                return 0;
            };
            let Ok(identity) = String::from_utf16(text) else {
                return 0;
            };
            if identity.is_empty() || identity.contains('\0') {
                return 0;
            }
            crate::diagnostics::event(
                "camera.ui.requested",
                serde_json::json!({"identity":identity,"running":context.running.get(),"autostart":data.tag == 2}),
            );
            if context.running.get()
                && !context.busy.get()
                && context
                    .selected_camera()
                    .is_some_and(|c| c.matches(&identity))
            {
                return 1;
            }
            *context.requested.borrow_mut() = Some(identity);
            context.pending_selection.set(true);
            context.auto_start.set(data.tag == 2);
            context.retry_pending.set(false);
            1
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            if id == CAMERAS as usize && (wparam >> 16) as u32 == CBN_SELCHANGE {
                context.load_modes();
            } else if (wparam >> 16) == 0 {
                context.command(id);
            }
            0
        }
        WM_SIZE => {
            context.layout();
            0
        }
        WM_DPICHANGED => {
            context.fonts.borrow_mut().update(window);
            unsafe {
                apply_font(context.fps.get(), context.fonts.borrow().heading);
            }
            theme::dpi_rect(window, lparam);
            context.layout();
            0
        }
        WM_TIMER => {
            context.tick();
            0
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            info.ptMinTrackSize.x = scale(window, 720);
            info.ptMinTrackSize.y = scale(window, 540);
            0
        }
        WM_CLOSE => {
            crate::diagnostics::event(
                "camera.ui.close",
                serde_json::json!({"painted_frames":context.painted.get(),"stats":*context.worker.stats.lock().unwrap()}),
            );
            let _ = context.worker.send(Action::Shutdown);
            theme::destroy_window(window);
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
pub fn run(config: &Path, role: Role, identity: Option<String>, start: bool) -> io::Result<()> {
    let lock_role = if role == Role::Server {
        "Server"
    } else {
        "Client"
    };
    let startup_deadline = std::time::Instant::now() + Duration::from_secs(3);
    let _instance = loop {
        if let Some(instance) = crate::instance::acquire(lock_role, "CameraPreview")? {
            break instance;
        }
        unsafe {
            let window = FindWindowW(wide(class(role)).as_ptr(), null());
            if !window.is_null() && IsWindowVisible(window) != 0 {
                if let Some(identity) = identity.as_ref() {
                    let text = wide(identity);
                    let data = CopyData {
                        tag: if start { 2 } else { 1 },
                        length: (text.len() * 2) as u32,
                        data: text.as_ptr().cast(),
                    };
                    SendMessageW(window, WM_COPYDATA, 0, (&data as *const CopyData) as isize);
                }
                if IsWindow(window) != 0 {
                    ShowWindow(window, SW_RESTORE);
                    let popup = GetLastActivePopup(window);
                    SetForegroundWindow(if IsWindowVisible(popup) != 0 {
                        popup
                    } else {
                        window
                    });
                    notify_parent_modal(role, window);
                    return Ok(());
                }
            }
        }
        if std::time::Instant::now() >= startup_deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Camera preview is still closing or belongs to another Windows session",
            ));
        }
        // The old window can already be gone while its worker and instance guard
        // are still closing. Acquire again instead of losing the reopen request.
        std::thread::sleep(Duration::from_millis(50));
    };
    camera::init_preview(config, role);
    unsafe {
        register(class(role), Some(procedure))?;
        register("USBBridge.Camera.Video.2", Some(video_procedure))?;
        let context = Box::new(Context {
            low_light: Cell::new(null_mut()),
            low_light_supported: Cell::new(None),
            low_light_confirmed: Cell::new(false),
            config: config.into(),
            role,
            window: Cell::new(null_mut()),
            camera: Cell::new(null_mut()),
            mode: Cell::new(null_mut()),
            video: Cell::new(null_mut()),
            video_buffer: RefCell::new(VideoBuffer::default()),
            camera_label: Cell::new(null_mut()),
            mode_label: Cell::new(null_mut()),
            refresh: Cell::new(null_mut()),
            play: Cell::new(null_mut()),
            privacy: Cell::new(null_mut()),
            fps: Cell::new(null_mut()),
            summary: Cell::new(null_mut()),
            status: Cell::new(null_mut()),
            fonts: RefCell::new(theme::Fonts::default()),
            worker: camera::Worker::new()?,
            devices: RefCell::new(Vec::new()),
            modes: RefCell::new(Vec::new()),
            auto_start: Cell::new(start && identity.is_some()),
            requested: RefCell::new(identity),
            busy: Cell::new(false),
            running: Cell::new(false),
            painted: Cell::new(0),
            painted_sequence: Cell::new(0),
            last_stats: RefCell::new(Stats::default()),
            last_update: Cell::new(std::time::Instant::now()),
            last_frame: Cell::new(std::time::Instant::now()),
            last_frames: Cell::new(0),
            status_key: RefCell::new(String::new()),
            status_error: RefCell::new(String::new()),
            pending_selection: Cell::new(false),
            startup_retry: Cell::new(false),
            retry_pending: Cell::new(false),
        });
        let window = CreateWindowExW(
            0,
            wide(class(role)).as_ptr(),
            wide(&title(role)).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            920,
            680,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            (&*context as *const Context).cast(),
        );
        if window.is_null() {
            return Err(io::Error::last_os_error());
        }
        context.window.set(window);
        theme::window(window);
        *context.fonts.borrow_mut() = theme::Fonts::new(window);
        let regular = context.fonts.borrow().regular;
        for (slot, class, text, style, id) in [
            (&context.camera_label, "STATIC", "Camera", 0, 910),
            (&context.mode_label, "STATIC", "Video mode", 0, 911),
            (
                &context.camera,
                "COMBOBOX",
                "",
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                CAMERAS,
            ),
            (
                &context.mode,
                "COMBOBOX",
                "",
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                MODES,
            ),
            (
                &context.refresh,
                "BUTTON",
                "Refresh cameras",
                WS_TABSTOP,
                RESCAN as i32,
            ),
            (
                &context.play,
                "BUTTON",
                "Start preview",
                WS_TABSTOP,
                PLAY as i32,
            ),
            (
                &context.privacy,
                "BUTTON",
                "Camera permissions",
                WS_TABSTOP,
                PRIVACY as i32,
            ),
            (&context.fps, "STATIC", "FPS: —", 0, 912),
            (
                &context.low_light,
                "BUTTON",
                "Low-light compensation (may reduce FPS)",
                WS_TABSTOP | BS_AUTOCHECKBOX as u32,
                LOW_LIGHT as i32,
            ),
            (&context.summary, "STATIC", "", 0, 913),
            (&context.status, "STATIC", "", 0, 914),
        ] {
            let child = control(window, class, crate::i18n::t(text), style, id as usize);
            slot.set(child);
            apply_font(child, regular);
        }
        apply_font(context.fps.get(), context.fonts.borrow().heading);
        let video = CreateWindowExW(
            0,
            wide("USBBridge.Camera.Video.2").as_ptr(),
            null(),
            WS_VISIBLE | WS_CHILD,
            0,
            0,
            0,
            0,
            window,
            915usize as HMENU,
            GetModuleHandleW(null()),
            (&*context as *const Context).cast(),
        );
        context.video.set(video);
        context.labels();
        context.layout();
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..std::mem::zeroed()
        };
        let available = GetMonitorInfoW(
            MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        ) != 0;
        let work = monitor.rcWork;
        let width = scale(window, 920).min(if available {
            (work.right - work.left - 24).max(1)
        } else {
            scale(window, 920)
        });
        let height = scale(window, 680).min(if available {
            (work.bottom - work.top - 24).max(1)
        } else {
            scale(window, 680)
        });
        SetWindowPos(
            window,
            null_mut(),
            if available {
                work.left + (work.right - work.left - width) / 2
            } else {
                0
            },
            if available {
                work.top + (work.bottom - work.top - height) / 2
            } else {
                0
            },
            width,
            height,
            SWP_NOZORDER,
        );
        ShowWindow(window, SW_SHOW);
        SetForegroundWindow(window);
        notify_parent_modal(role, window);
        SetTimer(window, 1, 33, None);
        context.command(RESCAN);
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let clean = context.worker.shutdown(Duration::from_secs(2));
        crate::diagnostics::event("camera.cleanup", serde_json::json!({"completed":clean}));
        // Closing this isolated process releases even a misbehaving camera graph without touching USB sharing.
        Ok(())
    }
}
