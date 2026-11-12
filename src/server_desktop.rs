//! Server dashboard. All inventory, health probes and elevation run off the UI thread.
use super::*;
use crate::host_status::Dashboard;

const UPDATE: u32 = WM_APP + 45;
const TOGGLE: usize = 301;
const RELOAD: usize = 302;
const APPLY: usize = 303;
const SERVER_ABOUT: usize = 304;
const SERVER_HIDE: usize = 305;
const SETTINGS: usize = 306;
const PUBLISH: usize = 307;
const LOCAL_IDLE: usize = 308;
const CAMERA_PREVIEW: usize = 309;
const DEVICE_CAMERA_PREVIEW: usize = 310;
const FORGET: usize = 311;
const VIDEO_SETTINGS: usize = 312;
const SERVER_EXIT_APPLICATION: usize = 313;
enum Event {
    Snapshot(u64, Result<Dashboard, String>),
    Finished(Result<(), String>, String, bool),
}
struct ServerUi {
    modal_camera: ModalCamera,
    quitting: Cell<bool>,
    images: RefCell<Option<crate::device_icons::Images>>,
    path: PathBuf,
    window: Cell<HWND>,
    tabs: Cell<HWND>,
    heading: Cell<HWND>,
    detail: Cell<HWND>,
    toggle: Cell<HWND>,
    list: Cell<HWND>,
    footer: Cell<HWND>,
    refresh: Cell<HWND>,
    apply: Cell<HWND>,
    publish: Cell<HWND>,
    preview: Cell<HWND>,
    labels: RefCell<Vec<HWND>>,
    inputs: RefCell<Vec<HWND>>,
    snapshot: RefCell<Option<Dashboard>>,
    displayed: RefCell<Vec<crate::host_status::DeviceRow>>,
    refreshing: Arc<AtomicBool>,
    working: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<Event>>>,
    fonts: RefCell<theme::Fonts>,
    refresh_ticket: Cell<u64>,
    feedback: RefCell<Feedback>,
    dirty: Cell<bool>,
    loading: Cell<bool>,
    checking: Cell<bool>,
    baseline: RefCell<Vec<String>>,
}
pub struct ServerWindow {
    window: HWND,
    context: Box<ServerUi>,
}
impl ServerWindow {
    pub fn create(path: &Path) -> io::Result<Self> {
        unsafe {
            let init = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES,
            };
            InitCommonControlsEx(&init);
            register("USBBridge.Server.2", Some(procedure))?;
            let context = Box::new(ServerUi {
                modal_camera: ModalCamera::default(),
                quitting: Cell::new(false),
                images: RefCell::new(None),
                path: path.into(),
                window: Cell::new(null_mut()),
                tabs: Cell::new(null_mut()),
                heading: Cell::new(null_mut()),
                detail: Cell::new(null_mut()),
                toggle: Cell::new(null_mut()),
                list: Cell::new(null_mut()),
                footer: Cell::new(null_mut()),
                refresh: Cell::new(null_mut()),
                apply: Cell::new(null_mut()),
                publish: Cell::new(null_mut()),
                preview: Cell::new(null_mut()),
                labels: RefCell::new(Vec::new()),
                inputs: RefCell::new(Vec::new()),
                snapshot: RefCell::new(None),
                displayed: RefCell::new(Vec::new()),
                refreshing: Arc::new(AtomicBool::new(false)),
                working: Arc::new(AtomicBool::new(false)),
                events: Arc::new(Mutex::new(Vec::new())),
                fonts: RefCell::new(theme::Fonts::default()),
                refresh_ticket: Cell::new(0),
                feedback: RefCell::new(Feedback::default()),
                dirty: Cell::new(false),
                loading: Cell::new(false),
                checking: Cell::new(false),
                baseline: RefCell::new(Vec::new()),
            });
            let menu = CreateMenu();
            let file = CreatePopupMenu();
            let server = CreatePopupMenu();
            let help = CreatePopupMenu();
            AppendMenuW(
                file,
                MF_STRING,
                SERVER_HIDE,
                wide(crate::i18n::t("Свернуть в трей")).as_ptr(),
            );
            AppendMenuW(file, MF_SEPARATOR, 0, null());
            AppendMenuW(
                file,
                MF_STRING,
                SERVER_EXIT_APPLICATION,
                wide(crate::i18n::t("Exit application")).as_ptr(),
            );
            AppendMenuW(
                server,
                MF_STRING,
                TOGGLE,
                wide(crate::i18n::t("Включить / отключить сервер")).as_ptr(),
            );
            AppendMenuW(
                server,
                MF_STRING,
                RELOAD,
                wide(crate::i18n::t("Обновить устройства")).as_ptr(),
            );
            AppendMenuW(
                server,
                MF_STRING,
                SETTINGS,
                wide(crate::i18n::t("Настройки и порты…")).as_ptr(),
            );
            AppendMenuW(
                server,
                MF_STRING,
                CAMERA_PREVIEW,
                wide(crate::i18n::t("Camera test…")).as_ptr(),
            );
            AppendMenuW(
                server,
                MF_STRING,
                VIDEO_SETTINGS,
                wide(crate::i18n::t("USB camera - receive video")).as_ptr(),
            );
            AppendMenuW(
                help,
                MF_STRING,
                SERVER_ABOUT,
                wide(crate::i18n::t("About")).as_ptr(),
            );
            for (submenu, text) in [
                (file, crate::i18n::t("Файл")),
                (server, crate::i18n::t("Сервер")),
                (help, crate::i18n::t("Справка")),
            ] {
                AppendMenuW(menu, MF_POPUP, submenu as usize, wide(text).as_ptr());
            }
            append_language_menu(menu);
            let window = CreateWindowExW(
                0,
                wide("USBBridge.Server.2").as_ptr(),
                wide(&Role::Server.window_title()).as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1000,
                600,
                null_mut(),
                menu,
                GetModuleHandleW(null()),
                (&*context as *const ServerUi).cast(),
            );
            if window.is_null() {
                DestroyMenu(menu);
                return Err(io::Error::last_os_error());
            }
            context.window.set(window);
            theme::window(window);
            *context.fonts.borrow_mut() = theme::Fonts::new(window);
            let ui_font = context.fonts.borrow().regular;
            let big_font = context.fonts.borrow().heading;
            let heading = control(
                window,
                "STATIC",
                crate::i18n::t("Проверка сервера…"),
                0,
                310,
            );
            context.heading.set(heading);
            apply_font(heading, big_font);
            let detail = control(window, "STATIC", "", 0, 311);
            context.detail.set(detail);
            apply_font(detail, ui_font);
            let toggle = control(
                window,
                "BUTTON",
                crate::i18n::t("Подождите…"),
                WS_TABSTOP | BS_PUSHBUTTON as u32,
                TOGGLE,
            );
            context.toggle.set(toggle);
            apply_font(toggle, ui_font);
            EnableWindow(toggle, 0);
            let tabs = control(
                window,
                "SysTabControl32",
                "",
                WS_TABSTOP | WS_CLIPSIBLINGS,
                312,
            );
            context.tabs.set(tabs);
            apply_font(tabs, ui_font);
            for (index, title) in [
                crate::i18n::t("Устройства и состояние"),
                crate::i18n::t("Настройки"),
            ]
            .iter()
            .enumerate()
            {
                let mut text = wide(title);
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: text.as_mut_ptr(),
                    ..std::mem::zeroed()
                };
                SendMessageW(
                    tabs,
                    TCM_INSERTITEMW,
                    index,
                    (&item as *const TCITEMW) as isize,
                );
            }
            let list = control(
                window,
                "SysListView32",
                "",
                WS_TABSTOP
                    | WS_BORDER
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | LVS_REPORT
                    | LVS_SINGLESEL
                    | LVS_SHOWSELALWAYS,
                313,
            );
            context.list.set(list);
            let images = crate::device_icons::Images::new(scale(window, 16))?;
            SendMessageW(
                list,
                LVM_SETIMAGELIST,
                LVSIL_SMALL as usize,
                images.0 as isize,
            );
            *context.images.borrow_mut() = Some(images);
            apply_font(list, ui_font);
            SendMessageW(
                list,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                0,
                (LVS_EX_FULLROWSELECT
                    | LVS_EX_DOUBLEBUFFER
                    | LVS_EX_LABELTIP
                    | LVS_EX_CHECKBOXES
                    | LVS_EX_SUBITEMIMAGES) as isize,
            );
            for (index, title) in [
                crate::i18n::t("Включено"),
                crate::i18n::t("Устройство"),
                crate::i18n::t("USB-порт"),
                crate::i18n::t("Состояние"),
                crate::i18n::t("Клиент (IP:TCP)"),
                crate::i18n::t("TCP-порт"),
            ]
            .iter()
            .enumerate()
            {
                let mut text = wide(title);
                let column = LVCOLUMNW {
                    mask: LVCF_TEXT | LVCF_WIDTH | LVCF_SUBITEM,
                    pszText: text.as_mut_ptr(),
                    cx: 120,
                    iSubItem: index as i32,
                    ..std::mem::zeroed()
                };
                SendMessageW(
                    list,
                    LVM_INSERTCOLUMNW,
                    index,
                    (&column as *const LVCOLUMNW) as isize,
                );
            }
            let footer = control(
                window,
                "STATIC",
                crate::i18n::t("Обновление списка USB…"),
                0,
                314,
            );
            context.footer.set(footer);
            apply_font(footer, ui_font);
            let refresh = control(
                window,
                "BUTTON",
                crate::i18n::t("Обновить"),
                WS_TABSTOP,
                RELOAD,
            );
            context.refresh.set(refresh);
            apply_font(refresh, ui_font);
            let publish = control(
                window,
                "BUTTON",
                crate::i18n::t("Опубликовать"),
                WS_TABSTOP,
                PUBLISH,
            );
            context.publish.set(publish);
            apply_font(publish, ui_font);
            EnableWindow(publish, 0);
            let preview = control(
                window,
                "BUTTON",
                crate::i18n::t("Camera test…"),
                WS_TABSTOP,
                CAMERA_PREVIEW,
            );
            context.preview.set(preview);
            apply_font(preview, ui_font);
            for (index, title) in [
                crate::i18n::t("Имя сервера"),
                crate::i18n::t("TCP-порт подключения (1–65535)"),
                crate::i18n::t("Порт поиска и списка (UDP/TCP, 1–65535)"),
                crate::i18n::t("Разрешённые IP клиентов (по одному на строку)"),
                crate::i18n::t("Серверный модуль"),
                crate::i18n::t("Automatic cleanup"),
                crate::i18n::t("Days without reconnection (1–36500)"),
            ]
            .iter()
            .enumerate()
            {
                let label = control(window, "STATIC", title, 0, 0);
                apply_font(label, ui_font);
                context.labels.borrow_mut().push(label);
                let (class, style) = match index {
                    1 | 2 | 6 => (
                        "EDIT",
                        WS_BORDER | WS_TABSTOP | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
                    ),
                    3 => (
                        "EDIT",
                        WS_BORDER
                            | WS_TABSTOP
                            | WS_VSCROLL
                            | ES_MULTILINE as u32
                            | ES_AUTOVSCROLL as u32,
                    ),
                    4 => (
                        "COMBOBOX",
                        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
                    ),
                    5 => ("BUTTON", WS_TABSTOP | BS_AUTOCHECKBOX as u32),
                    _ => ("EDIT", WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32),
                };
                let input = control(window, class, "", style, 320 + index);
                if index == 5 {
                    set_text(input, crate::i18n::t("Remove unplugged devices"));
                }
                apply_font(input, ui_font);
                if index == 4 {
                    for title in [
                        crate::i18n::t("Native — сервер Rust"),
                        crate::i18n::t("Usbipd — штатный сервер"),
                    ] {
                        SendMessageW(input, CB_ADDSTRING, 0, wide(title).as_ptr() as isize);
                    }
                } else {
                    SendMessageW(
                        input,
                        EM_SETLIMITTEXT,
                        if matches!(index, 1 | 2 | 6) { 5 } else { 2048 },
                        0,
                    );
                }
                context.inputs.borrow_mut().push(input);
            }
            let apply = control(
                window,
                "BUTTON",
                crate::i18n::t("Применить настройки"),
                WS_TABSTOP,
                APPLY,
            );
            context.apply.set(apply);
            apply_font(apply, ui_font);
            context.load_settings();
            context.layout();
            context.select_tab(0);
            SetWindowPos(
                window,
                null_mut(),
                0,
                0,
                scale(window, 1000),
                scale(window, 600),
                SWP_NOMOVE | SWP_NOZORDER,
            );
            SetTimer(window, 3, 3000, None);
            context.refresh(false);
            Ok(Self { window, context })
        }
    }
    pub fn show(&self, settings: bool) {
        unsafe {
            ShowWindow(self.window, SW_RESTORE);
            let popup = GetLastActivePopup(self.window);
            SetForegroundWindow(if IsWindowVisible(popup) != 0 {
                popup
            } else {
                self.window
            });
        }
        if settings {
            self.context.load_settings();
        }
        self.context.select_tab(usize::from(settings));
        self.context.refresh(false);
        theme::redraw(self.window, true);
    }
    pub(crate) fn can_quit(&self) -> bool {
        !self.context.working.load(Ordering::Acquire)
            && !self.context.dirty.get()
            && (unsafe { GetWindowLongPtrW(self.window, GWL_STYLE) as u32 & WS_DISABLED == 0 }
                || self.context.modal_camera.allows_quit(self.window))
    }
    pub(crate) fn set_quitting(&self, quitting: bool) {
        self.context.quitting.set(quitting);
        self.context.working.store(quitting, Ordering::Release);
        unsafe {
            EnableWindow(
                self.window,
                i32::from(!quitting && !self.context.modal_camera.active()),
            );
        }
        self.context
            .feedback
            .borrow_mut()
            .set(crate::i18n::t(if quitting {
                "Exiting application... Stopping USB connections and background tasks."
            } else {
                "Application exit failed or was canceled."
            }));
        self.context.paint_status();
    }
    pub fn can_reload(&self) -> bool {
        !self.context.working.load(Ordering::Acquire)
            && !self.context.dirty.get()
            && unsafe { GetWindowLongPtrW(self.window, GWL_STYLE) as u32 & WS_DISABLED == 0 }
    }
}
impl Drop for ServerWindow {
    fn drop(&mut self) {
        unsafe {
            self.context.modal_camera.close(self.window);
            let menu = GetMenu(self.window);
            SetMenu(self.window, null_mut());
            theme::destroy_menu(menu);
            DestroyWindow(self.window);
        }
    }
}
impl ServerUi {
    fn select_tab(&self, index: usize) {
        unsafe {
            SendMessageW(self.tabs.get(), TCM_SETCURSEL, index, 0);
        }
        self.show_tab(index);
    }
    fn show_tab(&self, index: usize) {
        crate::diagnostics::event("ui.server.tab", serde_json::json!({"tab":index}));
        unsafe {
            for window in [
                self.list.get(),
                self.refresh.get(),
                self.publish.get(),
                self.preview.get(),
            ] {
                ShowWindow(window, if index == 0 { SW_SHOW } else { SW_HIDE });
            }
            for window in self
                .labels
                .borrow()
                .iter()
                .chain(self.inputs.borrow().iter())
                .copied()
                .chain([self.apply.get()])
            {
                ShowWindow(window, if index == 1 { SW_SHOW } else { SW_HIDE });
            }
        }
    }
    fn layout(&self) {
        unsafe {
            let w = self.window.get();
            if self.tabs.get().is_null() {
                return;
            }
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(w, &mut r);
            let s = |n| scale(w, n);
            let width = r.right;
            let bottom = r.bottom;
            MoveWindow(self.heading.get(), s(24), s(22), width - s(310), s(36), 1);
            MoveWindow(self.detail.get(), s(26), s(68), width - s(310), s(40), 1);
            MoveWindow(self.toggle.get(), width - s(268), s(28), s(244), s(52), 1);
            MoveWindow(self.tabs.get(), s(24), s(124), width - s(48), s(36), 1);
            SendMessageW(
                self.tabs.get(),
                TCM_SETITEMSIZE,
                0,
                ((s(36) as u32) << 16 | s(224) as u32) as isize,
            );
            MoveWindow(
                self.list.get(),
                s(24),
                s(174),
                width - s(48),
                (bottom - s(286)).max(s(80)),
                1,
            );
            MoveWindow(
                self.publish.get(),
                width - s(376),
                bottom - s(94),
                s(232),
                s(34),
                1,
            );
            MoveWindow(self.preview.get(), s(24), bottom - s(94), s(200), s(34), 1);
            MoveWindow(
                self.refresh.get(),
                width - s(134),
                bottom - s(94),
                s(110),
                s(34),
                1,
            );
            MoveWindow(
                self.footer.get(),
                s(24),
                bottom - s(42),
                width - s(48),
                s(26),
                1,
            );
            self.layout_columns();
            let col = (width - s(48) - s(28)) / 2;
            let right = s(24) + col + s(28);
            let positions = [
                (s(24), s(176), col, s(28)),
                (s(24), s(242), col, s(28)),
                (right, s(242), col, s(28)),
                (s(24), s(308), col, s(88)),
                (right, s(176), col, s(150)),
                (right, s(308), col, s(28)),
                (right, s(366), col, s(28)),
            ];
            for (i, (x, y, cw, ch)) in positions.into_iter().enumerate() {
                MoveWindow(self.labels.borrow()[i], x, y, cw, s(22), 1);
                MoveWindow(self.inputs.borrow()[i], x, y + s(27), cw, ch, 1);
            }
            SendMessageW(
                self.inputs.borrow()[4],
                CB_SETITEMHEIGHT,
                usize::MAX,
                s(28) as isize,
            );
            SendMessageW(self.inputs.borrow()[4], CB_SETITEMHEIGHT, 0, s(28) as isize);
            MoveWindow(self.apply.get(), right, bottom - s(94), col, s(34), 1);
        }
    }
    fn load_settings(&self) {
        if self.dirty.get() {
            return;
        }
        self.loading.set(true);
        let result = (|| -> io::Result<()> {
            let host: HostSettings = settings::read(&self.path)?;
            let state: serde_json::Value =
                settings::read(&host.runtime.join("backend-state.json"))?;
            for (input, value) in self.inputs.borrow().iter().zip([
                host.name,
                settings::endpoint(&host.probe_address)?.1.to_string(),
                host.discovery_port.to_string(),
                host.allowed_ips
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\r\n"),
            ]) {
                set_text(*input, &value);
            }
            unsafe {
                SendMessageW(
                    self.inputs.borrow()[5],
                    BM_SETCHECK,
                    usize::from(host.auto_remove_unplugged),
                    0,
                );
                set_text(
                    self.inputs.borrow()[6],
                    &host.unplugged_retention_days.to_string(),
                );
                SendMessageW(
                    self.inputs.borrow()[4],
                    CB_SETCURSEL,
                    usize::from(state["DesiredBackend"] == "Usbipd"),
                    0,
                );
            }
            Ok(())
        })();
        if let Err(error) = result {
            set_text(self.footer.get(), &error.to_string());
        }
        self.loading.set(false);
        *self.baseline.borrow_mut() = self.settings_values();
        self.paint_status();
    }
    fn layout_columns(&self) {
        unsafe {
            let list = self.list.get();
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(list, &mut rect);
            let s = |n| scale(list, n);
            let captions = [
                crate::i18n::t("Включено"),
                crate::i18n::t("Устройство"),
                crate::i18n::t("USB-порт"),
                crate::i18n::t("Состояние"),
                crate::i18n::t("Клиент (IP:TCP)"),
                crate::i18n::t("TCP-порт"),
            ];
            let mut widths = [s(96), s(180), s(72), s(178), s(158), s(80)];
            let header = SendMessageW(list, LVM_GETHEADER, 0, 0) as HWND;
            let dc = GetDC(header);
            if !dc.is_null() {
                let font = SendMessageW(header, WM_GETFONT, 0, 0) as HGDIOBJ;
                let old = SelectObject(dc, font);
                for (index, caption) in captions.iter().enumerate() {
                    let value = wide(caption);
                    let mut size = std::mem::zeroed();
                    if GetTextExtentPoint32W(
                        dc,
                        value.as_ptr(),
                        (value.len() - 1) as i32,
                        &mut size,
                    ) != 0
                    {
                        widths[index] = widths[index].max(size.cx + s(32));
                    }
                }
                SelectObject(dc, old);
                ReleaseDC(header, dc);
            }
            widths[1] = widths[1].max(
                rect.right
                    - widths
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != 1)
                        .map(|(_, width)| *width)
                        .sum::<i32>(),
            );
            for (index, cx) in widths.into_iter().enumerate() {
                if SendMessageW(list, LVM_GETCOLUMNWIDTH, index, 0) != cx as isize {
                    SendMessageW(list, LVM_SETCOLUMNWIDTH, index, cx as isize);
                }
            }
        }
    }
    fn settings_values(&self) -> Vec<String> {
        self.inputs
            .borrow()
            .iter()
            .enumerate()
            .map(|(index, input)| {
                if index == 4 {
                    unsafe { SendMessageW(*input, CB_GETCURSEL, 0, 0) }.to_string()
                } else if index == 5 {
                    unsafe { SendMessageW(*input, BM_GETCHECK, 0, 0) }.to_string()
                } else {
                    window_text(*input).trim().replace("\r\n", "\n")
                }
            })
            .collect()
    }
    fn refresh(&self, manual: bool) {
        if manual {
            let ticket =
                self.refresh_ticket.get() + u64::from(!self.refreshing.load(Ordering::Acquire));
            self.feedback
                .borrow_mut()
                .begin(ticket, crate::i18n::t("Обновление устройств…"));
            self.paint_status();
            unsafe {
                UpdateWindow(self.footer.get());
                UpdateWindow(self.refresh.get());
            }
        }
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return;
        }
        let ticket = self.refresh_ticket.get() + 1;
        self.refresh_ticket.set(ticket);
        let path = self.path.clone();
        let events = self.events.clone();
        let flag = self.refreshing.clone();
        let window = self.window.get() as usize;
        std::thread::spawn(move || {
            let result = crate::host_status::collect(&path).map_err(|e| e.to_string());
            events.lock().unwrap().push(Event::Snapshot(ticket, result));
            flag.store(false, Ordering::Release);
            unsafe {
                PostMessageW(window as HWND, UPDATE, 0, 0);
            }
        });
    }
    fn paint_status(&self) {
        let busy = self.working.load(Ordering::Acquire);
        let ports_valid = {
            let inputs = self.inputs.borrow();
            inputs.len() < 3
                || (parse_port(&window_text(inputs[1])).is_ok()
                    && parse_port(&window_text(inputs[2])).is_ok())
        };
        let retention_valid = self.inputs.borrow().get(6).is_none_or(|w| {
            window_text(*w)
                .trim()
                .parse::<u32>()
                .is_ok_and(|n| (1..=36500).contains(&n))
        });
        if let Some(state) = &*self.snapshot.borrow() {
            let title = if self.quitting.get() {
                crate::i18n::t("Exiting application...")
            } else if busy {
                crate::i18n::t("Изменение состояния…")
            } else if state.running && state.enabled {
                crate::i18n::t("СЕРВЕР РАБОТАЕТ")
            } else if state.running {
                crate::i18n::t("ОСТАНОВКА СЕРВЕРА…")
            } else if !state.enabled {
                crate::i18n::t("СЕРВЕР ВЫКЛЮЧЕН")
            } else {
                crate::i18n::t("СЕРВЕР НЕ РАБОТАЕТ")
            };
            set_text(self.heading.get(), title);
            set_text(
                self.detail.get(),
                &crate::i18n::format(
                    "{}  •  TCP {}  •  Поиск UDP {}  •  USB: {}",
                    &[
                        ("0", state.backend.to_string()),
                        ("1", format!("{}", state.tcp_port)),
                        ("2", format!("{}", state.discovery_port)),
                        ("3", format!("{}", state.rows.len())),
                    ],
                ),
            );
            set_text(
                self.toggle.get(),
                if busy {
                    crate::i18n::t("Подождите…")
                } else if state.enabled {
                    crate::i18n::t("Отключить")
                } else {
                    crate::i18n::t("Включить")
                },
            );
            unsafe {
                EnableWindow(self.toggle.get(), i32::from(!busy));
            }
        } else {
            unsafe {
                EnableWindow(self.toggle.get(), 0);
            }
        }
        unsafe {
            EnableWindow(
                self.apply.get(),
                i32::from(!busy && self.dirty.get() && ports_valid && retention_valid),
            );
            EnableWindow(
                self.refresh.get(),
                i32::from(!busy && self.feedback.borrow().refresh.is_none()),
            );
            EnableWindow(self.preview.get(), i32::from(!busy));
            for window in self
                .labels
                .borrow()
                .iter()
                .chain(self.inputs.borrow().iter())
            {
                EnableWindow(*window, i32::from(!busy));
            }
            if self.inputs.borrow().len() == 7 {
                let inputs = self.inputs.borrow();
                let enabled = SendMessageW(inputs[5], BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
                EnableWindow(inputs[6], i32::from(!busy && enabled));
                EnableWindow(self.labels.borrow()[6], i32::from(!busy && enabled));
            }
            for id in [
                TOGGLE,
                RELOAD,
                SETTINGS,
                APPLY,
                PUBLISH,
                CAMERA_PREVIEW,
                SERVER_EXIT_APPLICATION,
            ] {
                EnableMenuItem(
                    GetMenu(self.window.get()),
                    id as u32,
                    MF_BYCOMMAND
                        | if busy
                            || (id == TOGGLE && self.snapshot.borrow().is_none())
                            || (id == RELOAD && self.feedback.borrow().refresh.is_some())
                        {
                            MF_GRAYED
                        } else {
                            MF_ENABLED
                        },
                );
            }
            InvalidateRect(self.heading.get(), null(), 1);
        }
        let feedback = self.feedback.borrow();
        let snapshot = self.snapshot.borrow();
        set_text(
            self.footer.get(),
            if self.quitting.get() {
                crate::i18n::t(
                    "Exiting application... Stopping USB connections and background tasks.",
                )
            } else if self.dirty.get() && !ports_valid {
                crate::i18n::t("Укажите порты от 1 до 65535")
            } else if self.dirty.get() && !retention_valid {
                crate::i18n::t("Days must be in 1..36500")
            } else if !feedback.text.is_empty() {
                &feedback.text
            } else {
                snapshot
                    .as_ref()
                    .and_then(|s| s.warning.as_deref())
                    .unwrap_or(crate::i18n::t("Готово. Список обновляется автоматически."))
            },
        );
        set_text(
            self.refresh.get(),
            if feedback.refresh.is_some() {
                crate::i18n::t("Обновление…")
            } else {
                crate::i18n::t("Обновить")
            },
        );
        self.publication_state();
    }
    fn selected_row(&self) -> Option<crate::host_status::DeviceRow> {
        let index = unsafe {
            SendMessageW(
                self.list.get(),
                LVM_GETNEXTITEM,
                usize::MAX,
                LVNI_SELECTED as isize,
            )
        };
        self.displayed.borrow().get(index as usize).cloned()
    }
    fn publication_state(&self) {
        let row = self.selected_row();
        let busy = self.working.load(Ordering::Acquire);
        set_text(
            self.publish.get(),
            if row.as_ref().is_some_and(|r| r.published) {
                crate::i18n::t("Снять публикацию")
            } else {
                crate::i18n::t("Опубликовать")
            },
        );
        unsafe {
            EnableWindow(
                self.publish.get(),
                i32::from(!busy && row.as_ref().is_some_and(|r| r.present || r.published)),
            );
        }
    }
    fn display_rows(&self, rows: &[crate::host_status::DeviceRow]) {
        unsafe {
            if *self.displayed.borrow() == rows {
                return;
            }
            let list = self.list.get();
            let structural = self
                .displayed
                .borrow()
                .iter()
                .map(|r| &r.identity)
                .ne(rows.iter().map(|r| &r.identity));
            let top = SendMessageW(list, LVM_GETTOPINDEX, 0, 0);
            let selected = SendMessageW(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
            let identity = self
                .displayed
                .borrow()
                .get(selected as usize)
                .map(|r| r.identity.clone());
            let redraw = theme::RedrawGuard::new(list);
            self.checking.set(true);
            if structural {
                SendMessageW(list, LVM_DELETEALLITEMS, 0, 0);
            }
            for (index, row) in rows.iter().enumerate() {
                for (column, value) in [
                    if row.local_when_idle {
                        crate::i18n::t("Да")
                    } else {
                        crate::i18n::t("Нет")
                    },
                    &row.name,
                    &row.busid,
                    crate::i18n::t(&row.state),
                    &row.client,
                    &row.tcp_port,
                ]
                .iter()
                .enumerate()
                {
                    let mut text = wide(value);
                    let item = LVITEMW {
                        mask: LVIF_TEXT | if column == 1 { LVIF_IMAGE } else { 0 },
                        iImage: row.kind.image(),
                        iItem: index as i32,
                        iSubItem: column as i32,
                        pszText: text.as_mut_ptr(),
                        ..std::mem::zeroed()
                    };
                    SendMessageW(
                        list,
                        if structural && column == 0 {
                            LVM_INSERTITEMW
                        } else {
                            LVM_SETITEMW
                        },
                        0,
                        (&item as *const LVITEMW) as isize,
                    );
                }
                let state = LVITEMW {
                    state: if row.local_when_idle {
                        2 << 12
                    } else {
                        1 << 12
                    },
                    stateMask: LVIS_STATEIMAGEMASK,
                    ..std::mem::zeroed()
                };
                SendMessageW(
                    list,
                    LVM_SETITEMSTATE,
                    index,
                    (&state as *const LVITEMW) as isize,
                );
                if identity.as_ref() == Some(&row.identity) {
                    let item = LVITEMW {
                        state: LVIS_SELECTED | LVIS_FOCUSED,
                        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
                        ..std::mem::zeroed()
                    };
                    SendMessageW(
                        list,
                        LVM_SETITEMSTATE,
                        index,
                        (&item as *const LVITEMW) as isize,
                    );
                }
            }
            *self.displayed.borrow_mut() = rows.to_vec();
            self.checking.set(false);
            if structural && top > 0 {
                SendMessageW(
                    list,
                    LVM_ENSUREVISIBLE,
                    (top as usize).min(rows.len().saturating_sub(1)),
                    0,
                );
            }
            drop(redraw);
            self.layout_columns();
        }
    }
    fn events(&self) {
        let pending = std::mem::take(&mut *self.events.lock().unwrap());
        for event in pending {
            match event {
                Event::Snapshot(ticket, Ok(state)) => {
                    self.feedback.borrow_mut().finish(
                        ticket,
                        state
                            .warning
                            .clone()
                            .map(|w| {
                                crate::i18n::format(
                                    "Обновление завершено: {w}",
                                    &[("w", w.to_string())],
                                )
                            })
                            .unwrap_or(crate::i18n::t("Обновление устройств завершено").into()),
                    );
                    self.display_rows(&state.rows);
                    *self.snapshot.borrow_mut() = Some(state);
                }
                Event::Snapshot(ticket, Err(error)) => {
                    self.feedback.borrow_mut().finish(
                        ticket,
                        crate::i18n::format(
                            "Ошибка обновления: {error}",
                            &[("error", error.to_string())],
                        ),
                    );
                    *self.snapshot.borrow_mut() = None;
                    set_text(self.heading.get(), crate::i18n::t("СТАТУС НЕДОСТУПЕН"));
                    if self.feedback.borrow().text.is_empty() {
                        self.feedback.borrow_mut().set(crate::i18n::format(
                            "Ошибка обновления: {error}",
                            &[("error", error.to_string())],
                        ));
                    }
                }
                Event::Finished(result, success, settings_changed) => {
                    crate::diagnostics::event(
                        "ui.server.operation.complete",
                        serde_json::json!({"error":result.as_ref().err()}),
                    );
                    if result.is_ok() && settings_changed {
                        self.dirty.set(false);
                        self.load_settings();
                    }
                    self.feedback.borrow_mut().set(
                        result
                            .as_ref()
                            .err()
                            .map(|e| crate::i18n::format("Ошибка: {e}", &[("e", e.to_string())]))
                            .unwrap_or(success),
                    );
                    if let Err(error) = result {
                        information(self.window.get(), &error);
                    }
                    self.refresh(false);
                }
            }
        }
        self.paint_status();
    }
    fn command(&self, id: usize) {
        if self.quitting.get() {
            return;
        }
        if self.working.load(Ordering::Acquire) && !matches!(id, SERVER_ABOUT | SERVER_HIDE) {
            return;
        }
        if let Some(language) = language_choice(id) {
            if self.dirty.get() {
                information(
                    self.window.get(),
                    crate::i18n::t("Save settings before changing language."),
                );
            } else if let Err(error) =
                crate::tray::change_language(&self.path, Role::Server, language)
            {
                information(self.window.get(), &error.to_string());
            }
            return;
        }
        crate::diagnostics::event(
            "ui.server.action",
            serde_json::json!({"id":id,"action":match id{TOGGLE=>"toggle",APPLY=>"apply_settings",RELOAD=>"refresh",SETTINGS=>"settings_tab",SERVER_ABOUT=>"about",SERVER_HIDE=>"hide",SERVER_EXIT_APPLICATION=>"exit_application",PUBLISH=>"publish_device",FORGET=>"forget_device",CAMERA_PREVIEW=>"camera_preview",DEVICE_CAMERA_PREVIEW=>"camera_test_device",_=>"other"},"device":self.selected_row()}),
        );
        match id {
            SERVER_EXIT_APPLICATION => {
                if let Err(error) = crate::tray::request_quit(Role::Server) {
                    information(self.window.get(), &error.to_string());
                }
            }
            VIDEO_SETTINGS => video_settings(self.window.get(), &self.path, Role::Server),
            FORGET => {
                if let Some(row) = self.selected_row().filter(|r| !r.present)
                    && self
                        .snapshot
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| s.presence_verified)
                {
                    self.run_request(
                        "Publish-Device.ps1",
                        serde_json::json!({"identity":row.identity,"publish":false,"forget":true}),
                    );
                }
            }
            CAMERA_PREVIEW | DEVICE_CAMERA_PREVIEW => {
                let identity = (id == DEVICE_CAMERA_PREVIEW)
                    .then(|| self.selected_row().map(|row| row.identity))
                    .flatten();
                if let Err(error) = crate::camera::launch(
                    &self.path,
                    Role::Server,
                    identity.as_deref(),
                    id == DEVICE_CAMERA_PREVIEW,
                ) {
                    information(self.window.get(), &error.to_string());
                }
            }
            LOCAL_IDLE => {
                if let Some(row) = self.selected_row() {
                    let local = !row.local_when_idle;
                    self.set_local_idle(row, local);
                }
            }
            PUBLISH => {
                if let Some(row) = self.selected_row().filter(|r| r.present || r.published) {
                    self.run_request(
                        "Publish-Device.ps1",
                        serde_json::json!({"identity":row.identity,"publish":!row.published}),
                    );
                }
            }
            SETTINGS => {
                self.load_settings();
                self.select_tab(1);
            }
            RELOAD => self.refresh(true),
            SERVER_ABOUT => about(self.window.get()),
            SERVER_HIDE => unsafe {
                ShowWindow(self.window.get(), SW_HIDE);
            },
            TOGGLE => {
                let enabled = self.snapshot.borrow().as_ref().map(|s| !s.enabled);
                if let Some(enabled) = enabled {
                    self.run_request(
                        "Set-ServerState.ps1",
                        serde_json::json!({"enabled":enabled}),
                    );
                }
            }
            APPLY => {
                let result = (|| -> io::Result<serde_json::Value> {
                    let inputs = self.inputs.borrow();
                    let name = window_text(inputs[0]).trim().to_string();
                    let tcp = parse_port(&window_text(inputs[1]))?;
                    let udp = parse_port(&window_text(inputs[2]))?;
                    if tcp == udp {
                        return Err(io::Error::other(crate::i18n::t(
                            "TCP-порт USB и порт поиска/списка устройств должны различаться",
                        )));
                    }
                    let peers = window_text(inputs[3])
                        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                        .filter(|v| !v.is_empty())
                        .map(|v| v.parse::<std::net::IpAddr>().map_err(io::Error::other))
                        .collect::<io::Result<Vec<_>>>()?;
                    if name.is_empty() || name.len() > 128 || peers.is_empty() {
                        return Err(io::Error::other(crate::i18n::t(
                            "Укажите имя сервера и хотя бы один IP клиента",
                        )));
                    }
                    let backend = if unsafe { SendMessageW(inputs[4], CB_GETCURSEL, 0, 0) } == 1 {
                        "Usbipd"
                    } else {
                        "Native"
                    };
                    let auto_remove = unsafe { SendMessageW(inputs[5], BM_GETCHECK, 0, 0) }
                        == BST_CHECKED as isize;
                    let days = window_text(inputs[6])
                        .trim()
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=36500).contains(n))
                        .ok_or_else(|| {
                            io::Error::other(crate::i18n::t("Days must be in 1..36500"))
                        })?;
                    Ok(
                        serde_json::json!({"name":name,"tcp_port":tcp,"discovery_port":udp,"allowed_ips":peers,"backend":backend,"auto_remove_unplugged":auto_remove,"unplugged_retention_days":days}),
                    )
                })();
                match result {
                    Ok(request) => self.run_request("Configure-Server.ps1", request),
                    Err(error) => information(self.window.get(), &error.to_string()),
                }
            }
            _ => {}
        }
    }
    fn set_local_idle(&self, row: crate::host_status::DeviceRow, enabled: bool) {
        if self.working.load(Ordering::Acquire) {
            return;
        }
        if self
            .snapshot
            .borrow()
            .as_ref()
            .is_none_or(|s| s.backend != "Native")
        {
            self.feedback.borrow_mut().set(crate::i18n::t(
                "Настройка локального использования применяется к модулю Native",
            ));
            self.paint_status();
            return;
        }
        crate::diagnostics::event(
            "ui.server.local_when_idle",
            serde_json::json!({"identity":row.identity,"enabled":enabled,"published":row.published}),
        );
        self.run_request("Publish-Device.ps1",serde_json::json!({"identity":row.identity,"publish":row.published,"local_when_idle":enabled}));
    }
    fn checkbox_changed(&self, change: &NMLISTVIEW) {
        if self.checking.get()
            || change.iItem < 0
            || change.uChanged & LVIF_STATE == 0
            || (change.uOldState ^ change.uNewState) & LVIS_STATEIMAGEMASK == 0
        {
            return;
        }
        let row = self.displayed.borrow().get(change.iItem as usize).cloned();
        if let Some(row) = row {
            let enabled = change.uNewState & LVIS_STATEIMAGEMASK == 2 << 12;
            if enabled == row.local_when_idle {
                return;
            }
            self.checking.set(true);
            let item = LVITEMW {
                state: if row.local_when_idle {
                    2 << 12
                } else {
                    1 << 12
                },
                stateMask: LVIS_STATEIMAGEMASK,
                ..unsafe { std::mem::zeroed() }
            };
            unsafe {
                SendMessageW(
                    self.list.get(),
                    LVM_SETITEMSTATE,
                    change.iItem as usize,
                    (&item as *const LVITEMW) as isize,
                );
            }
            self.checking.set(false);
            self.set_local_idle(row, enabled);
        }
    }
    fn popup(&self, lparam: LPARAM) {
        unsafe {
            let mut point = windows_sys::Win32::Foundation::POINT {
                x: (lparam as u32 & 0xffff) as i16 as i32,
                y: ((lparam as u32 >> 16) & 0xffff) as i16 as i32,
            };
            let keyboard = point.x == -1 && point.y == -1;
            let row = if keyboard {
                let selected = SendMessageW(
                    self.list.get(),
                    LVM_GETNEXTITEM,
                    usize::MAX,
                    LVNI_SELECTED as isize,
                );
                let mut rect: RECT = std::mem::zeroed();
                SendMessageW(
                    self.list.get(),
                    LVM_GETITEMRECT,
                    selected as usize,
                    (&mut rect as *mut RECT) as isize,
                );
                point.x = rect.left + scale(self.window.get(), 20);
                point.y = rect.bottom;
                ClientToScreen(self.list.get(), &mut point);
                self.selected_row()
            } else {
                let mut hit = LVHITTESTINFO {
                    pt: point,
                    ..std::mem::zeroed()
                };
                ScreenToClient(self.list.get(), &mut hit.pt);
                let index = SendMessageW(
                    self.list.get(),
                    LVM_SUBITEMHITTEST,
                    0,
                    (&mut hit as *mut LVHITTESTINFO) as isize,
                );
                if index >= 0 {
                    let mut state = LVITEMW {
                        state: 0,
                        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
                        ..std::mem::zeroed()
                    };
                    SendMessageW(
                        self.list.get(),
                        LVM_SETITEMSTATE,
                        usize::MAX,
                        (&state as *const LVITEMW) as isize,
                    );
                    state.state = LVIS_SELECTED | LVIS_FOCUSED;
                    SendMessageW(
                        self.list.get(),
                        LVM_SETITEMSTATE,
                        index as usize,
                        (&state as *const LVITEMW) as isize,
                    );
                    SetFocus(self.list.get());
                    self.displayed.borrow().get(index as usize).cloned()
                } else {
                    None
                }
            };
            let menu = CreatePopupMenu();
            crate::diagnostics::event(
                "ui.server.context_menu",
                serde_json::json!({"device":row,"keyboard":keyboard}),
            );
            AppendMenuW(
                menu,
                MF_STRING | MF_DISABLED | MF_GRAYED,
                0,
                wide(&Role::Server.header()).as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            let busy = self.working.load(Ordering::Acquire);
            let camera_device = row.is_some();
            if let Some(row) = row {
                AppendMenuW(
                    menu,
                    MF_STRING
                        | if !busy
                            && !row.present
                            && self
                                .snapshot
                                .borrow()
                                .as_ref()
                                .is_some_and(|s| s.presence_verified)
                        {
                            MF_ENABLED
                        } else {
                            MF_GRAYED
                        },
                    FORGET,
                    wide(crate::i18n::t("Remove unplugged device")).as_ptr(),
                );
                AppendMenuW(
                    menu,
                    MF_STRING
                        | if row.local_when_idle {
                            MF_CHECKED
                        } else {
                            MF_UNCHECKED
                        }
                        | if busy { MF_GRAYED } else { MF_ENABLED },
                    LOCAL_IDLE,
                    wide(crate::i18n::t("Использовать на сервере без клиента")).as_ptr(),
                );
                AppendMenuW(
                    menu,
                    MF_STRING | MF_DISABLED | MF_GRAYED,
                    0,
                    wide(&row.name).as_ptr(),
                );
                AppendMenuW(
                    menu,
                    MF_STRING
                        | if !busy && (row.present || row.published) {
                            MF_ENABLED
                        } else {
                            MF_GRAYED
                        },
                    PUBLISH,
                    wide(if row.published {
                        crate::i18n::t("Снять публикацию устройства")
                    } else {
                        crate::i18n::t("Опубликовать устройство")
                    })
                    .as_ptr(),
                );
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
            }
            AppendMenuW(
                menu,
                MF_STRING
                    | if busy || self.feedback.borrow().refresh.is_some() {
                        MF_GRAYED
                    } else {
                        MF_ENABLED
                    },
                RELOAD,
                wide(crate::i18n::t("Обновить список устройств")).as_ptr(),
            );
            AppendMenuW(
                menu,
                MF_STRING,
                SERVER_ABOUT,
                wide(crate::i18n::t("About")).as_ptr(),
            );
            AppendMenuW(
                menu,
                MF_STRING
                    | if camera_device && !busy {
                        MF_ENABLED
                    } else {
                        MF_GRAYED
                    },
                DEVICE_CAMERA_PREVIEW,
                wide(crate::i18n::t("Camera test…")).as_ptr(),
            );
            theme::menu(menu, false);
            let action = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                self.window.get(),
                null(),
            );
            theme::destroy_menu(menu);
            if action != 0 {
                self.command(action as usize);
            }
        }
    }
    fn run_request(&self, script: &'static str, request: serde_json::Value) {
        if self.working.swap(true, Ordering::AcqRel) {
            return;
        }
        let (progress, success) = match script {
            "Publish-Device.ps1" if request["forget"] == true => (
                crate::i18n::t("Removing device…"),
                crate::i18n::t("Device removed"),
            ),
            "Publish-Device.ps1" if request.get("local_when_idle").is_some() => (
                crate::i18n::t("Сохранение режима устройства…"),
                crate::i18n::t("Режим устройства сохранён"),
            ),
            "Publish-Device.ps1" => (
                crate::i18n::t("Изменение публикации устройства…"),
                crate::i18n::t("Публикация устройства изменена"),
            ),
            "Configure-Server.ps1" => (
                crate::i18n::t("Применение настроек сервера…"),
                crate::i18n::t("Настройки сервера сохранены"),
            ),
            _ => (
                crate::i18n::t("Изменение состояния сервера…"),
                crate::i18n::t("Состояние сервера изменено"),
            ),
        };
        self.feedback.borrow_mut().set(progress);
        self.paint_status();
        let config = self.path.clone();
        let events = self.events.clone();
        let working = self.working.clone();
        let window = self.window.get() as usize;
        std::thread::spawn(move || {
            let result = crate::control::host_request(&config, script, &request)
                .map(|_| ())
                .map_err(|e| e.to_string());
            events.lock().unwrap().push(Event::Finished(
                result,
                success.into(),
                script == "Configure-Server.ps1",
            ));
            working.store(false, Ordering::Release);
            unsafe {
                PostMessageW(window as HWND, UPDATE, 0, 0);
            }
        });
    }
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
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const ServerUi;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let ui = unsafe { &*pointer };
    // The health heading uses its semantic color, with the same dark background.
    if (message != WM_CTLCOLORSTATIC || lparam as HWND != ui.heading.get())
        && let Some(result) = theme::message(window, message, wparam, lparam)
    {
        return result;
    }
    match message {
        CAMERA_MODAL => {
            ui.modal_camera.begin(window, wparam as HWND, Role::Server);
            0
        }
        WM_MENUSELECT => {
            if wparam & 0xffff == SERVER_EXIT_APPLICATION && !ui.quitting.get() {
                set_text(
                    ui.footer.get(),
                    crate::i18n::t(
                        "Exit stops USB connections and background work. Windows startup settings are kept.",
                    ),
                );
            } else {
                ui.paint_status();
            }
            0
        }
        WM_CONTEXTMENU if wparam as HWND == ui.list.get() => {
            ui.popup(lparam);
            0
        }
        WM_DPICHANGED => {
            ui.fonts.borrow_mut().update(window);
            theme::dpi_rect(window, lparam);
            ui.layout();
            0
        }
        WM_SIZE => {
            ui.layout();
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            if (320..327).contains(&id) {
                if !ui.loading.get()
                    && ((wparam >> 16) as u32 == EN_CHANGE
                        || (id == 324 && (wparam >> 16) as u32 == CBN_SELCHANGE)
                        || (id == 325 && (wparam >> 16) as u32 == BN_CLICKED))
                {
                    ui.dirty.set(ui.settings_values() != *ui.baseline.borrow());
                    ui.paint_status();
                }
            } else {
                ui.command(id);
            }
            0
        }
        WM_NOTIFY => {
            let header = unsafe { &*(lparam as *const NMHDR) };
            if header.hwndFrom == ui.tabs.get() && header.code == TCN_SELCHANGE {
                let index = unsafe { SendMessageW(ui.tabs.get(), TCM_GETCURSEL, 0, 0) };
                if index == 1 {
                    ui.load_settings();
                }
                ui.show_tab(index as usize);
            }
            if header.hwndFrom == ui.list.get() && header.code == LVN_ITEMCHANGED {
                ui.checkbox_changed(unsafe { &*(lparam as *const NMLISTVIEW) });
                ui.publication_state();
            }
            if header.hwndFrom == ui.list.get() && header.code == NM_DBLCLK {
                let click = unsafe { &*(lparam as *const NMITEMACTIVATE) };
                if click.iItem >= 0
                    && click.iSubItem != 0
                    && !ui.working.load(Ordering::Acquire)
                    && let Some(row) = ui
                        .displayed
                        .borrow()
                        .get(click.iItem as usize)
                        .filter(|r| r.present && !r.published)
                        .cloned()
                {
                    crate::diagnostics::event(
                        "ui.server.double_click_share",
                        serde_json::json!({"identity":row.identity}),
                    );
                    ui.run_request(
                        "Publish-Device.ps1",
                        serde_json::json!({"identity":row.identity,"publish":true}),
                    );
                }
            }
            if let Some(result) = theme::notification(lparam) {
                return result;
            }
            0
        }
        WM_TIMER if wparam == 4 => {
            ui.modal_camera.tick(window);
            if ui.quitting.get() {
                unsafe {
                    EnableWindow(window, 0);
                }
            }
            0
        }
        WM_TIMER if wparam == 3 => {
            if unsafe { IsWindowVisible(window) } != 0 {
                ui.refresh(false);
            }
            0
        }
        UPDATE => {
            ui.events();
            0
        }
        WM_CTLCOLORSTATIC if lparam as HWND == ui.heading.get() => {
            let dc = wparam as HDC;
            let running = ui
                .snapshot
                .borrow()
                .as_ref()
                .is_some_and(|s| s.running && s.enabled);
            unsafe {
                SetTextColor(
                    dc,
                    if running {
                        theme::SUCCESS
                    } else {
                        theme::DANGER
                    },
                );
                SetBkColor(dc, theme::BACKGROUND);
                theme::background() as isize
            }
        }
        WM_CLOSE => {
            crate::diagnostics::event(
                "ui.window.hide",
                serde_json::json!({"role":"server","reason":"close_button"}),
            );
            unsafe {
                ShowWindow(window, SW_HIDE);
            }
            0
        }
        WM_GETMINMAXINFO => {
            let limits = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            limits.ptMinTrackSize.x = scale(window, 840);
            limits.ptMinTrackSize.y = scale(window, 580);
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(window, 3);
            }
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
