//! Native desktop controls. Network and driver operations run off the UI thread.
use crate::{
    agent, catalog, discovery, recovery,
    settings::{self, ClientSettings, HostSettings, SavedServer},
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, HiDpi::GetDpiForWindow, Shell::ShellExecuteW, WindowsAndMessaging::*},
};
#[path = "camera_desktop.rs"]
mod camera_desktop;
#[path = "server_desktop.rs"]
mod server_desktop;
pub use camera_desktop::run as camera_preview;
#[path = "desktop_theme.rs"]
pub(crate) mod theme;
pub use server_desktop::ServerWindow;
pub fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
const LANGUAGE_FIRST: usize = 600;
pub fn language_choice(id: usize) -> Option<crate::i18n::Language> {
    crate::i18n::LANGUAGES
        .get(id.checked_sub(LANGUAGE_FIRST)?)
        .map(|entry| entry.0)
}
pub(crate) fn append_language_menu(bar: HMENU) {
    unsafe {
        let menu = CreatePopupMenu();
        for (index, (language, name)) in crate::i18n::LANGUAGES.iter().enumerate() {
            AppendMenuW(
                menu,
                MF_STRING
                    | if *language == crate::i18n::language() {
                        MF_CHECKED
                    } else {
                        0
                    },
                LANGUAGE_FIRST + index,
                wide(name).as_ptr(),
            );
        }
        AppendMenuW(
            bar,
            MF_POPUP,
            menu as usize,
            wide(crate::i18n::t("Language")).as_ptr(),
        );
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
}
const CAMERA_MODAL: u32 = WM_APP + 46;
#[derive(Default)]
struct ModalCamera {
    preview: Cell<HWND>,
    restore_enabled: Cell<bool>,
}
impl ModalCamera {
    fn begin(&self, parent: HWND, preview: HWND, role: Role) {
        if preview.is_null() || self.preview.get() == preview {
            return;
        }
        unsafe {
            let mut class = [0; 64];
            let length = GetClassNameW(preview, class.as_mut_ptr(), class.len() as i32);
            let expected = if role == Role::Server {
                "USBBridge.Camera.Server.2"
            } else {
                "USBBridge.Camera.Client.2"
            };
            if String::from_utf16_lossy(&class[..length.max(0) as usize]) != expected {
                return;
            }
            self.restore_enabled
                .set(windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(parent) != 0);
            self.preview.set(preview);
            SetWindowLongPtrW(preview, GWLP_HWNDPARENT, parent as isize);
            EnableWindow(parent, 0);
            SetTimer(parent, 4, 100, None);
            SetForegroundWindow(preview);
        }
        crate::diagnostics::event(
            "ui.camera_modal.begin",
            serde_json::json!({"role":format!("{role:?}"),"preview":preview as usize}),
        );
    }
    fn tick(&self, parent: HWND) {
        let preview = self.preview.get();
        if preview.is_null() || unsafe { IsWindow(preview) } != 0 {
            return;
        }
        self.preview.set(null_mut());
        unsafe {
            KillTimer(parent, 4);
            if self.restore_enabled.replace(false) {
                EnableWindow(parent, 1);
            }
            SetForegroundWindow(GetLastActivePopup(parent));
        }
        crate::diagnostics::event("ui.camera_modal.end", serde_json::json!({}));
    }
    fn close(&self, parent: HWND) {
        unsafe {
            KillTimer(parent, 4);
            let preview = self.preview.replace(null_mut());
            if !preview.is_null() && IsWindow(preview) != 0 {
                PostMessageW(preview, WM_CLOSE, 0, 0);
            }
        }
    }
}
impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Self::Server => crate::i18n::t("Сервер"),
            Self::Client => crate::i18n::t("Клиент"),
        }
    }
    pub fn infer(path: &Path) -> Self {
        if settings::read::<HostSettings>(path).is_ok() {
            Self::Server
        } else {
            Self::Client
        }
    }
    pub fn header(self) -> String {
        format!("{} v.{}", self.label(), crate::APP_VERSION)
    }
    pub fn window_title(self) -> String {
        format!("{} — {}", crate::APP_NAME, self.header())
    }
}
fn open_repository(owner: HWND) {
    unsafe {
        ShellExecuteW(
            owner,
            wide("open").as_ptr(),
            wide(crate::REPOSITORY).as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        );
    }
}
pub(crate) fn information(owner: HWND, text: &str) {
    form_internal(
        owner,
        &crate::i18n::format(
            "Сообщение — {} v.{}",
            &[
                ("0", crate::APP_NAME.to_string()),
                ("1", crate::APP_VERSION.to_string()),
            ],
        ),
        Vec::new(),
        true,
        FormRules::Independent,
        Some(text.into()),
    );
}
fn window_text(window: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(window).max(0) as usize;
        let mut data = vec![0; length + 1];
        let n = GetWindowTextW(window, data.as_mut_ptr(), data.len() as i32);
        String::from_utf16_lossy(&data[..n.max(0) as usize])
    }
}
fn set_text(window: HWND, text: &str) {
    if window_text(window) == text {
        return;
    }
    unsafe {
        SetWindowTextW(window, wide(text).as_ptr());
    }
}
#[derive(Default)]
struct Feedback {
    text: String,
    refresh: Option<u64>,
}
impl Feedback {
    fn begin(&mut self, ticket: u64, text: &str) {
        self.text = text.into();
        self.refresh = Some(ticket);
        crate::diagnostics::event(
            "ui.refresh.begin",
            serde_json::json!({"ticket":ticket,"message":text}),
        );
    }
    fn finish(&mut self, ticket: u64, text: String) {
        if self.refresh == Some(ticket) {
            self.refresh = None;
            self.text = text;
            crate::diagnostics::event(
                "ui.refresh.complete",
                serde_json::json!({"ticket":ticket,"message":self.text}),
            );
        }
    }
    fn set(&mut self, text: impl Into<String>) {
        self.refresh = None;
        self.text = text.into();
    }
}
fn scale(window: HWND, value: i32) -> i32 {
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    value * (dpi as i32) / 96
}
unsafe fn control(parent: HWND, class: &str, text: &str, style: u32, id: usize) -> HWND {
    unsafe {
        let style = match class {
            // SS_OWNERDRAW keeps disabled labels readable on a dark surface.
            "STATIC" if id != 310 => (style & !0x1f) | 0x0d,
            "BUTTON" if style & 0xf != BS_AUTOCHECKBOX as u32 => {
                (style & !0xf) | BS_OWNERDRAW as u32
            }
            "COMBOBOX" => style | CBS_OWNERDRAWFIXED as u32 | CBS_HASSTRINGS as u32,
            "SysTabControl32" => style | TCS_FIXEDWIDTH,
            _ => style,
        };
        let window = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_CLIPSIBLINGS
                | style
                | if class == "SysListView32" {
                    WS_CLIPCHILDREN
                } else {
                    0
                },
            0,
            0,
            0,
            0,
            parent,
            id as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        theme::control(window, class);
        window
    }
}
unsafe fn register(class_name: &str, procedure: WNDPROC) -> io::Result<()> {
    unsafe {
        let instance = GetModuleHandleW(null());
        let name = wide(class_name);
        let class = WNDCLASSW {
            lpfnWndProc: procedure,
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: LoadIconW(instance, 101usize as *const u16),
            hbrBackground: theme::background(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&class) == 0 && windows_sys::Win32::Foundation::GetLastError() != 1410 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
fn font(window: HWND) -> HFONT {
    unsafe {
        CreateFontW(
            -scale(window, 14),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Segoe UI").as_ptr(),
        )
    }
}
unsafe fn apply_font(window: HWND, font: HFONT) {
    unsafe {
        SendMessageW(window, WM_SETFONT, font as usize, 1);
    }
}

pub enum FieldKind {
    Text,
    Password,
    Number,
    Multiline,
    Choice(Vec<String>),
}
pub struct Field {
    pub label: String,
    pub value: String,
    pub kind: FieldKind,
}
impl Field {
    fn text(label: &str, value: impl ToString) -> Self {
        Self {
            label: label.into(),
            value: value.to_string(),
            kind: FieldKind::Text,
        }
    }
    fn number(label: &str, value: u16) -> Self {
        Self {
            label: format!("{label} (1–65535)"),
            value: value.to_string(),
            kind: FieldKind::Number,
        }
    }
}
struct Form {
    title: String,
    fields: Vec<Field>,
    inputs: RefCell<Vec<HWND>>,
    labels: RefCell<Vec<HWND>>,
    rules: FormRules,
    result: RefCell<Option<Vec<String>>>,
    about: bool,
    fonts: RefCell<theme::Fonts>,
    message: Option<String>,
}
#[derive(Clone, Copy, Default)]
enum FormRules {
    #[default]
    Independent,
    Proxy,
    Video,
}
impl FormRules {
    fn enabled(self, index: usize, choices: &[isize]) -> bool {
        match self {
            Self::Independent => true,
            Self::Video => index == 0 || choices.first() == Some(&1),
            Self::Proxy => match index {
                0 => true,
                1 | 2 => choices.first().is_some_and(|mode| *mode > 0),
                _ => choices.first().is_some_and(|mode| *mode > 0) && choices.get(2) == Some(&1),
            },
        }
    }
}
impl Form {
    fn ports_valid(&self) -> bool {
        self.fields
            .iter()
            .zip(self.inputs.borrow().iter())
            .all(|(field, input)| {
                !matches!(field.kind, FieldKind::Number) || parse_port(&window_text(*input)).is_ok()
            })
    }
    fn layout(&self, window: HWND) {
        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(window, &mut rect);
            let s = |n| scale(window, n);
            let width = rect.right - s(40);
            if self.about {
                MoveWindow(GetDlgItem(window, 0), s(20), s(20), width, s(140), 1);
                MoveWindow(
                    GetDlgItem(window, FORM_REPO as i32),
                    s(20),
                    rect.bottom - s(60),
                    s(240),
                    s(34),
                    1,
                );
            } else {
                let mut y = s(20);
                for (index, field) in self.fields.iter().enumerate() {
                    MoveWindow(self.labels.borrow()[index], s(20), y, width, s(20), 1);
                    let h = match field.kind {
                        FieldKind::Multiline => 78,
                        FieldKind::Choice(_) => 150,
                        _ => 30,
                    };
                    MoveWindow(
                        self.inputs.borrow()[index],
                        s(20),
                        y + s(25),
                        width,
                        s(h),
                        1,
                    );
                    if matches!(field.kind, FieldKind::Choice(_)) {
                        SendMessageW(
                            self.inputs.borrow()[index],
                            CB_SETITEMHEIGHT,
                            usize::MAX,
                            s(28) as isize,
                        );
                        SendMessageW(
                            self.inputs.borrow()[index],
                            CB_SETITEMHEIGHT,
                            0,
                            s(28) as isize,
                        );
                    }
                    y += s(if matches!(field.kind, FieldKind::Multiline) {
                        120
                    } else {
                        68
                    });
                }
                MoveWindow(
                    GetDlgItem(window, FORM_OK as i32),
                    rect.right - s(252),
                    rect.bottom - s(52),
                    s(112),
                    s(34),
                    1,
                );
            }
            MoveWindow(
                GetDlgItem(window, FORM_CANCEL as i32),
                rect.right - s(128),
                rect.bottom - s(if self.about { 60 } else { 52 }),
                s(108),
                s(34),
                1,
            );
        }
    }
    fn update_enabled(&self) {
        let inputs = self.inputs.borrow();
        let choices: Vec<_> = inputs
            .iter()
            .map(|h| unsafe { SendMessageW(*h, CB_GETCURSEL, 0, 0) })
            .collect();
        for (index, (input, label)) in inputs.iter().zip(self.labels.borrow().iter()).enumerate() {
            let enabled = i32::from(self.rules.enabled(index, &choices));
            unsafe {
                EnableWindow(*input, enabled);
                EnableWindow(*label, enabled);
            }
        }
        if let Some(input) = inputs.first() {
            unsafe {
                EnableWindow(
                    GetDlgItem(GetParent(*input), FORM_OK as i32),
                    i32::from(self.ports_valid()),
                );
            }
        }
    }
}
const FORM_OK: usize = 1;
const FORM_CANCEL: usize = 2;
const FORM_REPO: usize = 3;
const FORM_ABOUT: usize = 4;
unsafe extern "system" fn form_procedure(
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
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Form;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let form = unsafe { &*pointer };
    if let Some(result) = theme::message(window, message, wparam, lparam) {
        return result;
    }
    match message {
        DM_GETDEFID => ((DC_HASDEFID as usize) << 16 | FORM_OK) as isize,
        WM_DPICHANGED => {
            form.fonts.borrow_mut().update(window);
            theme::dpi_rect(window, lparam);
            form.layout(window);
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            if id >= 100 && (wparam >> 16) as u32 == EN_CHANGE {
                form.update_enabled();
                return 0;
            }
            if id >= 100 && (wparam >> 16) as u32 == CBN_SELCHANGE {
                form.update_enabled();
                crate::diagnostics::event(
                    "ui.form.choice",
                    serde_json::json!({
                        "title":form.title,"field":form.fields.get(id - 100).map(|f| &f.label),
                        "choice":unsafe { SendMessageW(lparam as HWND, CB_GETCURSEL, 0, 0) }
                    }),
                );
                return 0;
            }
            if id <= 4 {
                crate::diagnostics::event(
                    "ui.form.action",
                    serde_json::json!({"title":form.title,"action":match id{FORM_OK=>"apply",FORM_CANCEL=>"cancel",FORM_REPO=>"repository",FORM_ABOUT=>"about",_=>"other"}}),
                );
            }
            match id {
                FORM_OK => {
                    if !form.ports_valid() {
                        return 0;
                    }
                    *form.result.borrow_mut() = Some(
                        form.inputs
                            .borrow()
                            .iter()
                            .enumerate()
                            .map(|(i, h)| match form.fields[i].kind {
                                FieldKind::Choice(_) => {
                                    unsafe { SendMessageW(*h, CB_GETCURSEL, 0, 0) }.to_string()
                                }
                                _ => window_text(*h),
                            })
                            .collect(),
                    );
                    theme::destroy_window(window);
                    0
                }
                FORM_CANCEL => {
                    theme::destroy_window(window);
                    0
                }
                FORM_REPO => {
                    open_repository(window);
                    0
                }
                FORM_ABOUT => {
                    about(window);
                    0
                }
                _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
            }
        }
        WM_CLOSE => {
            theme::destroy_window(window);
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
fn form(owner: HWND, title: &str, fields: Vec<Field>, about: bool) -> Option<Vec<String>> {
    form_with_rules(owner, title, fields, about, FormRules::Independent)
}
fn form_with_rules(
    owner: HWND,
    title: &str,
    fields: Vec<Field>,
    about: bool,
    rules: FormRules,
) -> Option<Vec<String>> {
    form_internal(owner, title, fields, about, rules, None)
}
fn form_internal(
    owner: HWND,
    title: &str,
    fields: Vec<Field>,
    about: bool,
    rules: FormRules,
    message: Option<String>,
) -> Option<Vec<String>> {
    crate::diagnostics::event(
        "ui.form.open",
        serde_json::json!({"title":title,"fields":fields.iter().map(|f|f.label.clone()).collect::<Vec<_>>()}),
    );
    unsafe {
        let existing = GetLastActivePopup(owner);
        if existing != owner && IsWindowVisible(existing) != 0 {
            ShowWindow(existing, SW_RESTORE);
            SetForegroundWindow(existing);
            return None;
        }
        if register("USBBridge.Form.2", Some(form_procedure)).is_err() {
            return None;
        }
        let context = Box::new(Form {
            title: title.into(),
            fields,
            inputs: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
            rules,
            result: RefCell::new(None),
            about,
            fonts: RefCell::new(theme::Fonts::default()),
            message,
        });
        let height = if context.about {
            270
        } else {
            125 + context
                .fields
                .iter()
                .map(|f| {
                    if matches!(f.kind, FieldKind::Multiline) {
                        120
                    } else {
                        68
                    }
                })
                .sum::<i32>()
        };
        let menu = if context.about {
            null_mut()
        } else {
            let bar = CreateMenu();
            let help = CreatePopupMenu();
            AppendMenuW(
                help,
                MF_STRING,
                FORM_ABOUT,
                wide(crate::i18n::t("About")).as_ptr(),
            );
            AppendMenuW(
                bar,
                MF_POPUP,
                help as usize,
                wide(crate::i18n::t("Справка")).as_ptr(),
            );
            bar
        };
        let window = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            wide("USBBridge.Form.2").as_ptr(),
            wide(title).as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            scale(owner, 530),
            scale(owner, height + if about { 0 } else { 24 }),
            owner,
            menu,
            GetModuleHandleW(null()),
            (&*context as *const Form).cast(),
        );
        if window.is_null() {
            if !menu.is_null() {
                DestroyMenu(menu);
            }
            return None;
        }
        theme::window(window);
        *context.fonts.borrow_mut() = theme::Fonts::new(window);
        let ui_font = context.fonts.borrow().regular;
        let mut y = 18;
        if context.about {
            let label = control(
                window,
                "STATIC",
                &context.message.clone().unwrap_or_else(|| crate::i18n::format("{}\r\nВерсия {}\r\n\r\nКлиент и сервер USB по сети.\r\nЛицензия исходников: MIT.\r\n{}", &[("0", crate::APP_NAME.to_string()), ("1", crate::APP_VERSION.to_string()), ("2", crate::REPOSITORY.to_string())])),
                0,
                0,
            );
            MoveWindow(
                label,
                scale(window, 20),
                scale(window, 18),
                scale(window, 480),
                scale(window, 140),
                1,
            );
            apply_font(label, ui_font);
            if context.message.is_none() {
                let link = control(
                    window,
                    "BUTTON",
                    crate::i18n::t("Репозиторий на GitHub"),
                    WS_TABSTOP,
                    FORM_REPO,
                );
                MoveWindow(
                    link,
                    scale(window, 20),
                    scale(window, 172),
                    scale(window, 250),
                    scale(window, 30),
                    1,
                );
                apply_font(link, ui_font);
            }
        } else {
            for (index, field) in context.fields.iter().enumerate() {
                let label = control(window, "STATIC", &field.label, 0, 0);
                MoveWindow(
                    label,
                    scale(window, 20),
                    scale(window, y),
                    scale(window, 480),
                    scale(window, 20),
                    1,
                );
                apply_font(label, ui_font);
                context.labels.borrow_mut().push(label);
                let (class, style, h) = match &field.kind {
                    FieldKind::Multiline => (
                        "EDIT",
                        WS_BORDER
                            | WS_TABSTOP
                            | WS_VSCROLL
                            | ES_MULTILINE as u32
                            | ES_AUTOVSCROLL as u32,
                        78,
                    ),
                    FieldKind::Choice(_) => (
                        "COMBOBOX",
                        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
                        120,
                    ),
                    FieldKind::Number => (
                        "EDIT",
                        WS_BORDER | WS_TABSTOP | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
                        26,
                    ),
                    FieldKind::Text => ("EDIT", WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32, 26),
                    FieldKind::Password => (
                        "EDIT",
                        WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
                        26,
                    ),
                };
                let input = control(window, class, &field.value, style, 100 + index);
                MoveWindow(
                    input,
                    scale(window, 20),
                    scale(window, y + 21),
                    scale(window, 476),
                    scale(window, h),
                    1,
                );
                apply_font(input, ui_font);
                if let FieldKind::Choice(choices) = &field.kind {
                    for choice in choices {
                        SendMessageW(input, CB_ADDSTRING, 0, wide(choice).as_ptr() as isize);
                    }
                    SendMessageW(input, CB_SETCURSEL, field.value.parse().unwrap_or(0), 0);
                } else {
                    SendMessageW(
                        input,
                        EM_SETLIMITTEXT,
                        if matches!(field.kind, FieldKind::Number) {
                            5
                        } else {
                            2048
                        },
                        0,
                    );
                }
                context.inputs.borrow_mut().push(input);
                y += if matches!(field.kind, FieldKind::Multiline) {
                    112
                } else {
                    60
                };
            }
            let save = control(
                window,
                "BUTTON",
                crate::i18n::t("Применить"),
                WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
                FORM_OK,
            );
            MoveWindow(
                save,
                scale(window, 278),
                scale(window, y + 4),
                scale(window, 108),
                scale(window, 30),
                1,
            );
            apply_font(save, ui_font);
        }
        let cancel = control(
            window,
            "BUTTON",
            if about {
                crate::i18n::t("Закрыть")
            } else {
                crate::i18n::t("Отмена")
            },
            WS_TABSTOP,
            FORM_CANCEL,
        );
        MoveWindow(
            cancel,
            scale(window, 396),
            scale(window, if about { 172 } else { y + 4 }),
            scale(window, 100),
            scale(window, 30),
            1,
        );
        apply_font(cancel, ui_font);
        context.layout(window);
        context.update_enabled();
        EnableWindow(owner, 0);
        ShowWindow(window, SW_SHOW);
        SetForegroundWindow(window);
        if let Some(first) = context.inputs.borrow().first() {
            SetFocus(*first);
        }
        let mut message = std::mem::zeroed();
        let mut quit = false;
        while IsWindow(window) != 0 {
            let received = GetMessageW(&mut message, null_mut(), 0, 0);
            if received <= 0 {
                quit = received == 0;
                break;
            }
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        // Also retire the modal window if GetMessage failed or consumed WM_QUIT.
        // Its procedure must not outlive the stack-owned Form context.
        if IsWindow(window) != 0 {
            theme::destroy_window(window);
        }
        if quit {
            PostQuitMessage(message.wParam as i32);
        }
        EnableWindow(owner, 1);
        SetForegroundWindow(owner);
        context.result.borrow_mut().take()
    }
}
pub fn about(owner: HWND) {
    form(
        owner,
        &format!("About — {} v.{}", crate::APP_NAME, crate::APP_VERSION),
        Vec::new(),
        true,
    );
}
fn parse_port(value: &str) -> io::Result<u16> {
    let value = value.trim();
    if !value.is_empty()
        && value.bytes().all(|c| c.is_ascii_digit())
        && let Ok(port) = value.parse::<u16>()
        && port != 0
    {
        return Ok(port);
    }
    Err(io::Error::other(crate::i18n::t(
        "Порт должен быть от 1 до 65535",
    )))
}
fn video_source_options(
    cameras: &[crate::camera::Camera],
    saved: &str,
) -> (Vec<String>, Vec<String>, usize) {
    let mut labels = vec![crate::i18n::t("Test pattern").into()];
    let mut identities = vec!["test-pattern".to_owned()];
    labels.extend(cameras.iter().map(|c| c.name.clone()));
    identities.extend(cameras.iter().map(|c| c.stable_identity()));
    let selected = if saved == "test-pattern" {
        0
    } else if let Some(index) = cameras.iter().position(|c| c.matches(saved)) {
        index + 1
    } else {
        // Applying another setting while a camera is absent must retain its source.
        labels.push(format!("{}: {saved}", crate::i18n::t("Source unavailable")));
        identities.push(saved.to_owned());
        identities.len() - 1
    };
    (labels, identities, selected)
}
pub fn video_settings(owner: HWND, path: &Path, role: Role) {
    let result = (|| -> io::Result<()> {
        let receive = role == Role::Server;
        let file = if receive {
            crate::video::receiver_path(path)
        } else {
            crate::video::sender_path(path)
        };
        let old = if file.exists() {
            Some(settings::read::<serde_json::Value>(&file)?)
        } else {
            None
        };
        let enabled = old
            .as_ref()
            .is_some_and(|v| v["enabled"].as_bool() == Some(true));
        let mut camera_sources = Vec::new();
        let title = crate::i18n::t(if receive {
            "USB camera - receive video"
        } else {
            "Send video"
        });
        let mode_list = [(640u16, 480u16), (1280, 720), (1920, 1080), (3840, 2160)];
        let mut fields = vec![Field {
            label: crate::i18n::t("Enable and start with Windows").into(),
            value: usize::from(enabled).to_string(),
            kind: FieldKind::Choice(vec![
                crate::i18n::t("Disabled").into(),
                crate::i18n::t("Enabled").into(),
            ]),
        }];
        if receive {
            let saved = old.as_ref();
            fields.push(Field::text(
                crate::i18n::t("Camera name"),
                saved
                    .and_then(|v| v["camera"]["name"].as_str())
                    .unwrap_or("4k web camera"),
            ));
            let width = saved
                .and_then(|v| v["camera"]["width"].as_u64())
                .unwrap_or(1280) as u16;
            let height = saved
                .and_then(|v| v["camera"]["height"].as_u64())
                .unwrap_or(720) as u16;
            fields.push(Field {
                label: crate::i18n::t("Video mode").into(),
                value: mode_list
                    .iter()
                    .position(|m| *m == (width, height))
                    .unwrap_or(1)
                    .to_string(),
                kind: FieldKind::Choice(
                    mode_list
                        .iter()
                        .map(|(w, h)| format!("{w} x {h} - 30 FPS"))
                        .collect(),
                ),
            });
            fields.push(Field::number(
                crate::i18n::t("Video TCP port"),
                saved
                    .and_then(|v| v["video_port"].as_u64())
                    .unwrap_or(13251) as u16,
            ));
        } else {
            let cameras = crate::camera::list()?;
            let saved = old.as_ref();
            let source = saved
                .and_then(|v| v["camera"].as_str())
                .unwrap_or("test-pattern");
            let (choices, sources, selected) = video_source_options(&cameras, source);
            camera_sources = sources;
            fields.push(Field {
                label: crate::i18n::t("Video source").into(),
                value: selected.to_string(),
                kind: FieldKind::Choice(choices),
            });
            let config: ClientSettings = settings::read(path)?;
            let default = config
                .server
                .rsplit_once(':')
                .map(|(host, _)| format!("{host}:13251"))
                .unwrap_or_else(|| "127.0.0.1:13251".into());
            fields.push(Field::text(
                crate::i18n::t("Video server address"),
                saved.and_then(|v| v["server"].as_str()).unwrap_or(&default),
            ));
            fields.push(Field {
                label: crate::i18n::t("JPEG quality (1-100)").into(),
                value: saved
                    .and_then(|v| v["quality"].as_u64())
                    .unwrap_or(85)
                    .to_string(),
                kind: FieldKind::Text,
            });
        }
        fields.push(Field {
            label: crate::i18n::t("Connection key (16-128 characters; blank keeps current)").into(),
            value: String::new(),
            kind: FieldKind::Password,
        });
        let Some(values) = form_with_rules(
            owner,
            &format!("{title} v.{}", crate::APP_VERSION),
            fields,
            false,
            FormRules::Video,
        ) else {
            return Ok(());
        };
        let enabled = values[0] == "1";
        let key_file = if values[4].is_empty() {
            old.as_ref()
                .and_then(|v| v["key_file"].as_str())
                .map(PathBuf::from)
        } else {
            if !(16..=128).contains(&values[4].len()) || values[4].chars().any(char::is_control) {
                return Err(io::Error::other(crate::i18n::t(
                    "Connection key must contain 16-128 characters",
                )));
            }
            Some(crate::credentials::save_at(
                &file
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("video-credentials"),
                &crate::proxy::Credentials {
                    username: "USBBridgeVideo".into(),
                    password: values[4].clone(),
                },
            )?)
        };
        if enabled && key_file.is_none() {
            return Err(io::Error::other(crate::i18n::t(
                "Enter the same connection key on both computers",
            )));
        }
        let key_file = key_file.unwrap_or_else(|| file.with_extension("unused.key"));
        let request = if receive {
            let host: HostSettings = settings::read(path)?;
            let selected = values[2]
                .parse::<usize>()
                .ok()
                .and_then(|n| mode_list.get(n))
                .ok_or_else(|| io::Error::other("invalid camera mode"))?;
            let camera = crate::uvc::Config {
                name: values[1].trim().into(),
                serial: old
                    .as_ref()
                    .and_then(|v| v["camera"]["serial"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("UBC-{}", uuid::Uuid::new_v4().simple())),
                width: selected.0,
                height: selected.1,
                fps: 30,
                test_pattern: false,
            };
            let value = crate::video::ReceiverSettings {
                enabled,
                camera,
                usb_port: old
                    .as_ref()
                    .and_then(|v| v["usb_port"].as_u64())
                    .unwrap_or(13250) as u16,
                video_port: parse_port(&values[3])?,
                allowed_ips: host.allowed_ips,
                key_file,
            };
            value.validate()?;
            serde_json::to_value(value).map_err(io::Error::other)?
        } else {
            let source = values[1].parse::<usize>().map_err(io::Error::other)?;
            let camera = camera_sources
                .get(source)
                .ok_or_else(|| io::Error::other("invalid video source selection"))?
                .clone();
            let value = crate::video::SenderSettings {
                enabled,
                server: settings::normalize_endpoint(values[2].trim(), 13251)?,
                camera,
                quality: values[3].parse().map_err(io::Error::other)?,
                key_file,
            };
            value.validate()?;
            serde_json::to_value(value).map_err(io::Error::other)?
        };
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let result = video_apply(&path, role, request);
            match result {
                Ok(()) => information(
                    null_mut(),
                    crate::i18n::t(
                        "Video settings applied. The selected mode starts with Windows.",
                    ),
                ),
                Err(error) => information(null_mut(), &error.to_string()),
            }
        });
        Ok(())
    })();
    if let Err(error) = result {
        information(owner, &error.to_string());
    }
}
pub fn video_apply(path: &Path, role: Role, value: serde_json::Value) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    let config = if role == Role::Server {
        crate::video::receiver_path(path)
    } else {
        crate::video::sender_path(path)
    };
    let request = config.with_extension(format!("{}.local.request.json", uuid::Uuid::new_v4()));
    settings::save(&request, &value)?;
    let runtime = std::env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::other("runtime missing"))?
        .to_path_buf();
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let result = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(runtime.join("Set-VideoMode.ps1"))
        .arg("-Role")
        .arg(if role == Role::Server {
            "Receive"
        } else {
            "Send"
        })
        .arg("-Config")
        .arg(&config)
        .arg("-RequestFile")
        .arg(&request)
        .arg("-InteractiveUser")
        .arg(user)
        .arg("-Elevate")
        .creation_flags(0x08000000)
        .status();
    let _ = std::fs::remove_file(&request);
    if !result?.success() {
        return Err(io::Error::other(crate::i18n::t(
            "Video configuration failed; check the video log",
        )));
    }
    Ok(())
}
pub fn server_settings(owner: HWND, path: &Path) {
    let result = (|| -> io::Result<()> {
        let host: HostSettings = settings::read(path)?;
        let state: serde_json::Value = settings::read(&host.runtime.join("backend-state.json"))?;
        let backend = state["DesiredBackend"].as_str().unwrap_or("Native");
        let fields = vec![
            Field::text(crate::i18n::t("Имя сервера"), &host.name),
            Field::number(
                crate::i18n::t("TCP-порт подключения"),
                settings::endpoint(&host.probe_address)?.1,
            ),
            Field::number(
                crate::i18n::t("Порт поиска и списка устройств (UDP/TCP)"),
                host.discovery_port,
            ),
            Field {
                label: crate::i18n::t("Разрешённые IP клиентов (по одному на строку)").into(),
                value: host
                    .allowed_ips
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\r\n"),
                kind: FieldKind::Multiline,
            },
            Field {
                label: crate::i18n::t("Серверный модуль").into(),
                value: if backend == "Usbipd" { "1" } else { "0" }.into(),
                kind: FieldKind::Choice(vec![
                    crate::i18n::t("Native — сервер Rust").into(),
                    crate::i18n::t("Usbipd — штатный сервер").into(),
                ]),
            },
        ];
        let Some(values) = form(owner, &Role::Server.window_title(), fields, false) else {
            return Ok(());
        };
        let tcp = parse_port(&values[1])?;
        let udp = parse_port(&values[2])?;
        let peers: Vec<std::net::IpAddr> = values[3]
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().map_err(io::Error::other))
            .collect::<io::Result<_>>()?;
        if values[0].trim().is_empty() || values[0].len() > 128 || peers.is_empty() {
            return Err(io::Error::other(crate::i18n::t(
                "Укажите имя сервера и хотя бы один IP клиента",
            )));
        }
        let request = path.with_extension(format!("{}.local.request.json", uuid::Uuid::new_v4()));
        settings::save(
            &request,
            &serde_json::json!({"name":values[0].trim(),"tcp_port":tcp,"discovery_port":udp,"allowed_ips":peers,"backend":if values[4]=="1"{"Usbipd"}else{"Native"}}),
        )?;
        let config = path.to_path_buf();
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let result = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(host.runtime.join("Configure-Server.ps1"))
                .arg("-Config")
                .arg(config)
                .arg("-RequestFile")
                .arg(&request)
                .arg("-Elevate")
                .creation_flags(0x08000000)
                .status();
            let reply = PathBuf::from(format!("{}.result.json", request.display()));
            let outcome = settings::read::<serde_json::Value>(&reply);
            let message=match (result,outcome){(Ok(status),Ok(value)) if status.success()&&value["Success"].as_bool()==Some(true)=>crate::i18n::t("Настройки сервера сохранены. Сервер перезапущен; клиенты восстановят подключения. Если менялся UDP-порт, укажите его и в настройках поиска клиента.").into(),(_,Ok(value))=>value["Error"].as_str().unwrap_or(crate::i18n::t("Не удалось применить настройки")).to_string(),(Err(e),_)=>e.to_string(),_=>crate::i18n::t("Не удалось применить настройки. Проверьте журнал сервера.").into()};
            information(null_mut(), &message);
            let _ = std::fs::remove_file(&request);
            let _ = std::fs::remove_file(&reply);
        });
        Ok(())
    })();
    if let Err(error) = result {
        information(owner, &error.to_string());
    }
}
const VIDEO_SETTINGS: usize = 130;
const REFRESH: u32 = WM_APP + 40;
const CONNECT: usize = 101;
const DISCONNECT: usize = 102;
const SEARCH: usize = 103;
const ADD_SERVER: usize = 104;
const PORTS: usize = 105;
const ABOUT: usize = 106;
const HIDE: usize = 107;
const REMOVE_SERVER: usize = 108;
const SELECT_SERVER: usize = 109;
const PROXY: usize = 110;
const CAMERA_PREVIEW: usize = 111;
const DEVICE_CAMERA_PREVIEW: usize = 112;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Selection {
    server: String,
    device: Option<String>,
}
struct Snapshot {
    ticket: u64,
    servers: Vec<catalog::Server>,
    found: Vec<discovery::FoundServer>,
    config: ClientSettings,
    report: Option<recovery::ClientReport>,
}
enum UiEvent {
    Snapshot(Box<Snapshot>),
    Notice(String),
    Error(String),
    RefreshError(u64, String),
}
#[derive(Default)]
struct Model {
    servers: Vec<catalog::Server>,
    found: Vec<discovery::FoundServer>,
    config: ClientSettings,
    report: Option<recovery::ClientReport>,
    items: HashMap<isize, Selection>,
    selected: Option<Selection>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct TreeRow {
    parent: Option<usize>,
    selection: Option<Selection>,
    text: String,
    image: i32,
}
impl Model {
    fn tree_rows(&self) -> Vec<TreeRow> {
        let mut rows = vec![TreeRow {
            parent: None,
            selection: None,
            image: 0,
            text: crate::i18n::format(
                "USB-серверы ({})",
                &[("0", format!("{}", self.servers.len()))],
            ),
        }];
        let mut servers: Vec<_> = self.servers.iter().collect();
        servers.sort_by_key(|s| (s.saved.name.to_lowercase(), s.saved.key()));
        for server in servers {
            let key = server.saved.key();
            let parent = rows.len();
            rows.push(TreeRow {
                parent: Some(0),
                image: 0,
                selection: Some(Selection {
                    server: key.clone(),
                    device: None,
                }),
                text: format!(
                    "{} ({}){}",
                    if server.saved.name.is_empty() {
                        crate::i18n::t("Сервер")
                    } else {
                        &server.saved.name
                    },
                    server.saved.server,
                    if server.saved.paused {
                        crate::i18n::t(" — paused")
                    } else if server.error.is_some() {
                        crate::i18n::t(" — недоступен")
                    } else {
                        ""
                    }
                ),
            });
            // Publication, presence and bus number never determine a device's position.
            let mut devices = std::collections::BTreeMap::new();
            if let Some(saved) = self
                .config
                .connections()
                .into_iter()
                .find(|s| s.matches(&server.saved))
            {
                for target in saved.devices {
                    devices.insert(
                        target.identity.to_lowercase(),
                        (
                            target.identity.clone(),
                            format!("USB {:04x}:{:04x}", target.vendor, target.product),
                            target.busid,
                            if target.enabled {
                                crate::i18n::t("Ожидание возвращения USB")
                            } else {
                                crate::i18n::t("Не подключено")
                            }
                            .to_string(),
                            crate::device_kind::DeviceKind::Usb,
                        ),
                    );
                }
            }
            for device in &server.devices {
                devices.insert(
                    device.path.to_lowercase(),
                    (
                        device.path.clone(),
                        catalog::device_name(device),
                        device.busid.clone(),
                        crate::i18n::t("Не подключено").into(),
                        crate::device_kind::DeviceKind::classify(
                            &catalog::device_name(device),
                            Some(device),
                        ),
                    ),
                );
            }
            if let Some(catalog) = &server.catalog {
                for row in &catalog.devices {
                    devices.insert(
                        row.identity.to_lowercase(),
                        (
                            row.identity.clone(),
                            row.name.clone(),
                            row.busid.clone(),
                            row.state.clone(),
                            row.kind,
                        ),
                    );
                }
            }
            let mut devices: Vec<_> = devices.into_values().collect();
            devices.sort_by_key(|(id, name, _, _, _)| (name.to_lowercase(), id.to_lowercase()));
            for (identity, name, busid, state, kind) in devices {
                let state = if server.saved.paused {
                    crate::i18n::t("Server connection paused")
                } else {
                    ClientUi::device_status(self, &server.saved, &identity)
                        .map(|d| phase_text(&d.phase))
                        .unwrap_or(crate::i18n::t(&state))
                };
                rows.push(TreeRow {
                    parent: Some(parent),
                    image: kind.image(),
                    text: format!("{name} [{busid}] — {state}"),
                    selection: Some(Selection {
                        server: key.clone(),
                        device: Some(identity),
                    }),
                });
            }
        }
        rows
    }
}
struct ClientUi {
    modal_camera: ModalCamera,
    path: PathBuf,
    window: Cell<HWND>,
    tree: Cell<HWND>,
    footer: Cell<HWND>,
    buttons: RefCell<Vec<HWND>>,
    model: RefCell<Model>,
    rebuilding: Cell<bool>,
    rendered: RefCell<Vec<TreeRow>>,
    refreshing: Arc<AtomicBool>,
    working: Arc<AtomicBool>,
    selection_generation: Arc<AtomicU64>,
    events: Arc<Mutex<Vec<UiEvent>>>,
    last_search: Cell<Option<std::time::Instant>>,
    fonts: RefCell<theme::Fonts>,
    refresh_ticket: Cell<u64>,
    feedback: RefCell<Feedback>,
}
pub struct ClientWindow {
    window: HWND,
    context: Box<ClientUi>,
    _images: crate::device_icons::Images,
}
impl ClientWindow {
    pub fn create(path: &Path) -> io::Result<Self> {
        unsafe {
            let init = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_TREEVIEW_CLASSES,
            };
            InitCommonControlsEx(&init);
            register("USBBridge.Client.2", Some(client_procedure))?;
            let context = Box::new(ClientUi {
                modal_camera: ModalCamera::default(),
                path: path.to_path_buf(),
                window: Cell::new(null_mut()),
                tree: Cell::new(null_mut()),
                footer: Cell::new(null_mut()),
                buttons: RefCell::new(Vec::new()),
                model: RefCell::new(Model {
                    selected: settings::read(&path.with_extension("ui.json")).ok(),
                    ..Default::default()
                }),
                rebuilding: Cell::new(false),
                rendered: RefCell::new(Vec::new()),
                refreshing: Arc::new(AtomicBool::new(false)),
                working: Arc::new(AtomicBool::new(false)),
                selection_generation: Arc::new(AtomicU64::new(0)),
                events: Arc::new(Mutex::new(Vec::new())),
                last_search: Cell::new(None),
                fonts: RefCell::new(theme::Fonts::default()),
                refresh_ticket: Cell::new(0),
                feedback: RefCell::new(Feedback::default()),
            });
            let menu = CreateMenu();
            let file = CreatePopupMenu();
            let device = CreatePopupMenu();
            let view = CreatePopupMenu();
            let help = CreatePopupMenu();
            for (id, text) in [
                (ADD_SERVER, crate::i18n::t("Добавить сервер по адресу…")),
                (PORTS, crate::i18n::t("Порты подключения…")),
                (PROXY, crate::i18n::t("Прокси сервера…")),
                (HIDE, crate::i18n::t("Свернуть в трей")),
            ] {
                AppendMenuW(file, MF_STRING, id, wide(text).as_ptr());
            }
            for (id, text) in [
                (CONNECT, crate::i18n::t("Подключить устройство")),
                (DISCONNECT, crate::i18n::t("Отключить устройство")),
                (VIDEO_SETTINGS, crate::i18n::t("Send video")),
            ] {
                AppendMenuW(device, MF_STRING, id, wide(text).as_ptr());
            }
            AppendMenuW(
                view,
                MF_STRING,
                SEARCH,
                wide(crate::i18n::t("Найти серверы / обновить")).as_ptr(),
            );
            AppendMenuW(
                view,
                MF_STRING,
                CAMERA_PREVIEW,
                wide(crate::i18n::t("Camera test…")).as_ptr(),
            );
            AppendMenuW(
                help,
                MF_STRING,
                ABOUT,
                wide(crate::i18n::t("About")).as_ptr(),
            );
            for (submenu, text) in [
                (file, crate::i18n::t("Файл")),
                (device, crate::i18n::t("Устройство")),
                (view, crate::i18n::t("Вид")),
                (help, crate::i18n::t("Справка")),
            ] {
                AppendMenuW(menu, MF_POPUP, submenu as usize, wide(text).as_ptr());
            }
            append_language_menu(menu);
            let window = CreateWindowExW(
                0,
                wide("USBBridge.Client.2").as_ptr(),
                wide(&Role::Client.window_title()).as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                850,
                560,
                null_mut(),
                menu,
                GetModuleHandleW(null()),
                (&*context as *const ClientUi).cast(),
            );
            if window.is_null() {
                DestroyMenu(menu);
                return Err(io::Error::last_os_error());
            }
            context.window.set(window);
            theme::window(window);
            *context.fonts.borrow_mut() = theme::Fonts::new(window);
            let ui_font = context.fonts.borrow().regular;
            for (id, label) in [
                (CONNECT, crate::i18n::t("Подключить")),
                (DISCONNECT, crate::i18n::t("Отключить")),
                (SEARCH, crate::i18n::t("Найти серверы")),
                (ADD_SERVER, crate::i18n::t("Добавить сервер")),
                (PORTS, crate::i18n::t("Порты…")),
                (PROXY, crate::i18n::t("Прокси…")),
                (CAMERA_PREVIEW, crate::i18n::t("Camera test…")),
            ] {
                let button = control(window, "BUTTON", label, WS_TABSTOP, id);
                apply_font(button, ui_font);
                context.buttons.borrow_mut().push(button);
            }
            let tree = control(
                window,
                "SysTreeView32",
                "",
                WS_TABSTOP
                    | WS_BORDER
                    | TVS_HASBUTTONS
                    | TVS_HASLINES
                    | TVS_LINESATROOT
                    | TVS_SHOWSELALWAYS
                    | TVS_DISABLEDRAGDROP,
                201,
            );
            context.tree.set(tree);
            apply_font(tree, ui_font);
            SendMessageW(
                tree,
                TVM_SETEXTENDEDSTYLE,
                TVS_EX_DOUBLEBUFFER as usize,
                TVS_EX_DOUBLEBUFFER as isize,
            );
            let images = crate::device_icons::Images::new(scale(window, 16))?;
            SendMessageW(
                tree,
                TVM_SETIMAGELIST,
                TVSIL_NORMAL as usize,
                images.0 as isize,
            );
            let footer = control(window, "STATIC", crate::i18n::t("Поиск серверов…"), 0, 202);
            context.footer.set(footer);
            apply_font(footer, ui_font);
            context.layout();
            SetWindowPos(
                window,
                null_mut(),
                0,
                0,
                scale(window, 850),
                scale(window, 560),
                SWP_NOMOVE | SWP_NOZORDER,
            );
            SetTimer(window, 2, 3000, None);
            context.refresh(true);
            Ok(Self {
                window,
                context,
                _images: images,
            })
        }
    }
    pub fn show(&self, search: bool) {
        crate::diagnostics::event(
            "ui.window.show",
            serde_json::json!({"role":"client","search":search}),
        );
        unsafe {
            ShowWindow(self.window, SW_RESTORE);
            let popup = GetLastActivePopup(self.window);
            SetForegroundWindow(if IsWindowVisible(popup) != 0 {
                popup
            } else {
                self.window
            });
        }
        self.context.refresh(search);
        theme::redraw(self.window, true);
    }
    pub fn settings(&self) {
        self.show(false);
        self.context.connection_form(false);
    }
    pub fn can_reload(&self) -> bool {
        !self.context.working.load(Ordering::Acquire)
            && unsafe { GetWindowLongPtrW(self.window, GWL_STYLE) as u32 & WS_DISABLED == 0 }
    }
}
impl Drop for ClientWindow {
    fn drop(&mut self) {
        self.context.modal_camera.close(self.window);
        unsafe {
            let menu = GetMenu(self.window);
            SetMenu(self.window, null_mut());
            theme::destroy_menu(menu);
            DestroyWindow(self.window);
        }
    }
}
impl ClientUi {
    fn layout(&self) {
        unsafe {
            if self.tree.get().is_null() {
                return;
            }
            let w = self.window.get();
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(w, &mut rect);
            let margin = scale(w, 20);
            let gap = scale(w, 8);
            let height = scale(w, 34);
            let dc = GetDC(w);
            let old = SelectObject(dc, self.fonts.borrow().regular);
            let mut x = margin;
            let mut y = margin;
            for button in self.buttons.borrow().iter() {
                let value = wide(&window_text(*button));
                let mut size = std::mem::zeroed();
                GetTextExtentPoint32W(dc, value.as_ptr(), value.len() as i32 - 1, &mut size);
                let width = (size.cx + scale(w, 28)).max(scale(w, 84));
                if x + width > rect.right - margin && x > margin {
                    x = margin;
                    y += height + gap;
                }
                MoveWindow(*button, x, y, width, height, 1);
                x += width + gap;
            }
            SelectObject(dc, old);
            ReleaseDC(w, dc);
            let top = y + height + scale(w, 16);
            let footer = scale(w, 66);
            MoveWindow(
                self.tree.get(),
                margin,
                top,
                (rect.right - 2 * margin).max(80),
                (rect.bottom - top - footer - scale(w, 18)).max(60),
                1,
            );
            SendMessageW(self.tree.get(), TVM_SETITEMHEIGHT, scale(w, 28) as usize, 0);
            MoveWindow(
                self.footer.get(),
                margin,
                rect.bottom - footer,
                rect.right - 2 * margin,
                scale(w, 52),
                1,
            );
        }
    }
    fn refresh(&self, search: bool) {
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return;
        }
        let ticket = self.refresh_ticket.get() + 1;
        self.refresh_ticket.set(ticket);
        let search = search
            || self
                .last_search
                .get()
                .is_none_or(|t| t.elapsed() > Duration::from_secs(20));
        if search {
            self.last_search.set(Some(std::time::Instant::now()));
        }
        let path = self.path.clone();
        let prior = self.model.borrow().found.clone();
        let previous = self.model.borrow().servers.clone();
        let events = self.events.clone();
        let busy = self.refreshing.clone();
        let window = self.window.get() as usize;
        std::thread::spawn(move || {
            let result = (|| -> io::Result<Snapshot> {
                let config = match settings::read::<ClientSettings>(&path) {
                    Ok(c) => c,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => ClientSettings::default(),
                    Err(e) => return Err(e),
                };
                config.validate()?;
                let found = if search {
                    discovery::find(config.discovery_port, &[], Duration::from_secs(2))?
                } else {
                    prior
                };
                let servers = catalog::merge(&config, found.clone());
                let catalog_port = config.discovery_port;
                let workers: Vec<_> = servers
                    .into_iter()
                    .map(|mut server| {
                        let previous = previous
                            .iter()
                            .find(|s| s.saved.matches(&server.saved))
                            .cloned();
                        std::thread::spawn(move || {
                            let mut last = None;
                            for address in &server.addresses {
                                match crate::client::list_with_proxy(
                                    address,
                                    server.saved.proxy.as_ref(),
                                    Duration::from_secs(3),
                                ) {
                                    Ok(devices) => {
                                        server.devices = devices;
                                        server.saved.server = address.clone();
                                        last = None;
                                        break;
                                    }
                                    Err(e) => last = Some(e.to_string()),
                                }
                            }
                            server.catalog =
                                crate::management::fetch(&server.saved, catalog_port).ok();
                            if let Some(catalog) = &server.catalog {
                                if server.saved.server_id.is_none() {
                                    server.saved.server_id = Some(catalog.server_id.clone());
                                }
                                server.saved.name = catalog.name.clone();
                            } else if let Some(previous) = previous {
                                // A failed status request must not remove or rename known rows.
                                server.catalog = previous.catalog.map(|mut catalog| {
                                    for row in &mut catalog.devices {
                                        row.state = crate::i18n::t("Состояние уточняется").into();
                                    }
                                    catalog
                                });
                                if server.saved.server_id.is_none() {
                                    server.saved.server_id = previous.saved.server_id;
                                }
                                if server.saved.name.is_empty() {
                                    server.saved.name = previous.saved.name;
                                }
                            }
                            server.error = last;
                            server
                        })
                    })
                    .collect();
                let servers = workers.into_iter().filter_map(|w| w.join().ok()).collect();
                let report = settings::read::<recovery::ClientReport>(&agent::status_path(&path))
                    .ok()
                    .map(|mut report| {
                        report.invalidate_stale(recovery::timestamp());
                        report
                    });
                Ok(Snapshot {
                    ticket,
                    servers,
                    found,
                    config,
                    report,
                })
            })();
            events.lock().unwrap().push(match result {
                Ok(snapshot) => UiEvent::Snapshot(Box::new(snapshot)),
                Err(e) => UiEvent::RefreshError(ticket, e.to_string()),
            });
            busy.store(false, Ordering::Release);
            unsafe {
                PostMessageW(window as HWND, REFRESH, 0, 0);
            }
        });
    }
    fn request_refresh(&self) {
        let ticket =
            self.refresh_ticket.get() + u64::from(!self.refreshing.load(Ordering::Acquire));
        self.feedback.borrow_mut().begin(
            ticket,
            crate::i18n::t("Поиск серверов и обновление устройств…"),
        );
        self.details();
        unsafe {
            UpdateWindow(self.footer.get());
        }
        self.refresh(true);
    }
    fn selected(&self) -> Option<(catalog::Server, Option<crate::wire::Device>, Option<String>)> {
        let model = self.model.borrow();
        let selection = model.selected.as_ref()?;
        let server = model
            .servers
            .iter()
            .find(|s| s.saved.key() == selection.server)?
            .clone();
        let device = selection.device.as_ref().and_then(|id| {
            server
                .devices
                .iter()
                .find(|d| d.path.eq_ignore_ascii_case(id))
                .cloned()
        });
        Some((server, device, selection.device.clone()))
    }
    fn device_status<'a>(
        model: &'a Model,
        server: &SavedServer,
        identity: &str,
    ) -> Option<&'a recovery::DeviceStatus> {
        let report = model.report.as_ref()?;
        let connection = report
            .servers
            .iter()
            .find(|s| s.key == server.key())
            .map(|s| &s.status)
            .or_else(|| {
                if report.selected.server.eq_ignore_ascii_case(&server.server) {
                    Some(&report.selected)
                } else {
                    None
                }
            })?;
        connection
            .devices
            .iter()
            .find(|d| d.identity.eq_ignore_ascii_case(identity))
    }
    fn details(&self) {
        let selected = self.selected();
        let working = self.working.load(Ordering::Acquire);
        let refreshing = self.feedback.borrow().refresh.is_some();
        let (connect, disconnect, text) = if let Some((server, device, identity)) = selected {
            let model = self.model.borrow();
            let saved = model
                .config
                .connections()
                .into_iter()
                .find(|s| s.matches(&server.saved));
            let server_node = identity.is_none();
            let server_paused = saved.as_ref().is_some_and(|s| s.paused);
            let remembered = saved
                .as_ref()
                .is_some_and(|s| s.devices.iter().any(|d| d.enabled));
            let server_connected = model
                .report
                .as_ref()
                .and_then(|r| r.servers.iter().find(|s| s.key == server.saved.key()))
                .is_some_and(|s| s.status.devices.iter().any(|d| d.port.is_some()));
            let target = identity.as_ref().and_then(|id| {
                model
                    .config
                    .connections()
                    .into_iter()
                    .find(|s| s.matches(&server.saved))
                    .and_then(|s| {
                        s.devices
                            .into_iter()
                            .find(|d| d.identity.eq_ignore_ascii_case(id))
                    })
            });
            let live = identity
                .as_ref()
                .and_then(|id| Self::device_status(&model, &server.saved, id));
            let connected = live.is_some_and(|d| d.port.is_some());
            let text = if let Some(id) = identity {
                let metadata = server.catalog.as_ref().and_then(|c| {
                    c.devices
                        .iter()
                        .find(|r| r.identity.eq_ignore_ascii_case(&id))
                });
                let phase = live.map(|d| phase_text(&d.phase)).unwrap_or_else(|| {
                    metadata
                        .map(|r| crate::i18n::t(&r.state))
                        .unwrap_or(crate::i18n::t("Не используется"))
                });
                format!(
                    "{} — {phase}\r\n{}",
                    server.saved.name,
                    metadata
                        .map(|r| crate::i18n::format(
                            "{} — USB-порт {}{}",
                            &[
                                ("0", r.name.to_string()),
                                ("1", r.busid.to_string()),
                                (
                                    "2",
                                    (if !r.published {
                                        crate::i18n::t("; включите публикацию на сервере")
                                    } else {
                                        ""
                                    })
                                    .to_string()
                                )
                            ]
                        ))
                        .unwrap_or_else(|| device.as_ref().map(catalog::device_name).unwrap_or(id))
                )
            } else {
                format!(
                    "{}  •  {}\r\n{}",
                    server.saved.name,
                    server.saved.server,
                    if server_paused {
                        crate::i18n::t(
                            "Server disconnected on this client. Device choices are preserved.",
                        )
                    } else {
                        server.error.as_deref().unwrap_or(crate::i18n::t(
                            "Выберите USB-устройство. Подключения сохраняются автоматически.",
                        ))
                    }
                )
            };
            (
                if server_node {
                    server_paused && remembered
                } else {
                    device.is_some() && !connected
                },
                if server_node {
                    server_connected || (!server_paused && remembered)
                } else {
                    connected || target.is_some_and(|t| t.enabled)
                },
                text,
            )
        } else {
            (
                false,
                false,
                crate::i18n::t("Выберите сервер и USB-устройство. Можно добавить сервер вручную по IP или имени.")
                    .into(),
            )
        };
        unsafe {
            let buttons = self.buttons.borrow();
            for button in buttons.iter() {
                let id = GetDlgCtrlID(*button) as usize;
                let enabled = match id {
                    CONNECT => connect && !working,
                    DISCONNECT => disconnect && !working,
                    PORTS | PROXY => self.selected().is_some() && !working,
                    ABOUT | HIDE => true,
                    SEARCH => !working && !refreshing,
                    _ => !working,
                };
                EnableWindow(*button, i32::from(enabled));
            }
            let menu = GetMenu(self.window.get());
            EnableMenuItem(
                menu,
                CONNECT as u32,
                MF_BYCOMMAND
                    | if connect && !working {
                        MF_ENABLED
                    } else {
                        MF_GRAYED
                    },
            );
            for id in [
                SEARCH,
                ADD_SERVER,
                PORTS,
                PROXY,
                SELECT_SERVER,
                REMOVE_SERVER,
                CAMERA_PREVIEW,
            ] {
                let enabled = !working
                    && (id != SEARCH || !refreshing)
                    && (!matches!(id, PORTS | PROXY | SELECT_SERVER | REMOVE_SERVER)
                        || self.selected().is_some());
                EnableMenuItem(
                    menu,
                    id as u32,
                    MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
                );
            }
            EnableMenuItem(
                menu,
                DISCONNECT as u32,
                MF_BYCOMMAND
                    | if disconnect && !working {
                        MF_ENABLED
                    } else {
                        MF_GRAYED
                    },
            );
        }
        let feedback = self.feedback.borrow();
        if let Some(button) = self
            .buttons
            .borrow()
            .iter()
            .find(|h| unsafe { GetDlgCtrlID(**h) as usize == SEARCH })
        {
            set_text(
                *button,
                if refreshing {
                    crate::i18n::t("Обновление…")
                } else {
                    crate::i18n::t("Найти серверы")
                },
            );
        }
        set_text(
            self.footer.get(),
            &if feedback.text.is_empty() {
                text
            } else {
                format!("{}\r\n{}", feedback.text, text.replace("\r\n", " · "))
            },
        );
    }
    fn render(&self) {
        let rows = self.model.borrow().tree_rows();
        if *self.rendered.borrow() == rows {
            self.details();
            return;
        }
        let structural = {
            let previous = self.rendered.borrow();
            previous.len() != rows.len()
                || previous
                    .iter()
                    .zip(&rows)
                    .any(|(a, b)| a.selection != b.selection || a.parent != b.parent)
        };
        self.rebuilding.set(true);
        let redraw = theme::RedrawGuard::new(self.tree.get());
        if !structural {
            let previous = self.rendered.borrow();
            let model = self.model.borrow();
            for (_, row) in previous
                .iter()
                .zip(&rows)
                .filter(|(a, b)| a.text != b.text || a.image != b.image)
            {
                let handle = match &row.selection {
                    Some(selection) => model
                        .items
                        .iter()
                        .find(|(_, s)| *s == selection)
                        .map(|(h, _)| *h)
                        .unwrap_or(0),
                    None => unsafe {
                        SendMessageW(self.tree.get(), TVM_GETNEXTITEM, TVGN_ROOT as usize, 0)
                    },
                };
                let mut text = wide(&row.text);
                let item = TVITEMW {
                    mask: TVIF_TEXT | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
                    iImage: row.image,
                    iSelectedImage: row.image,
                    hItem: handle,
                    pszText: text.as_mut_ptr(),
                    ..Default::default()
                };
                unsafe {
                    SendMessageW(
                        self.tree.get(),
                        TVM_SETITEMW,
                        0,
                        (&item as *const TVITEMW) as isize,
                    );
                }
            }
        } else {
            let (selected, expanded, top, first) = {
                let model = self.model.borrow();
                let expanded: HashSet<_> = model
                    .items
                    .iter()
                    .filter(|(item, selection)| {
                        selection.device.is_none()
                            && unsafe {
                                SendMessageW(
                                    self.tree.get(),
                                    TVM_GETITEMSTATE,
                                    **item as usize,
                                    TVIS_EXPANDED as isize,
                                )
                            } & TVIS_EXPANDED as isize
                                != 0
                    })
                    .map(|(_, s)| s.server.clone())
                    .collect();
                let top = unsafe {
                    SendMessageW(
                        self.tree.get(),
                        TVM_GETNEXTITEM,
                        TVGN_FIRSTVISIBLE as usize,
                        0,
                    )
                };
                (
                    model.selected.clone(),
                    expanded,
                    model.items.get(&top).cloned(),
                    model.items.is_empty(),
                )
            };
            let horizontal = unsafe { GetScrollPos(self.tree.get(), SB_HORZ) };
            unsafe {
                SendMessageW(self.tree.get(), TVM_DELETEITEM, 0, TVI_ROOT);
            }
            self.model.borrow_mut().items.clear();
            let mut handles = Vec::new();
            let mut restore = 0;
            let mut restore_top = 0;
            for row in &rows {
                let parent = row.parent.map(|i| handles[i]).unwrap_or(TVI_ROOT);
                let item = self.insert(parent, &row.text, row.image, row.selection.clone());
                handles.push(item);
                if row.selection.is_some() && row.selection == selected {
                    restore = item;
                }
                if row.selection.is_some() && row.selection == top {
                    restore_top = item;
                }
            }
            for (row, item) in rows.iter().zip(&handles) {
                if row.parent.is_none()
                    || row.selection.as_ref().is_some_and(|s| {
                        s.device.is_none() && (first || expanded.contains(&s.server))
                    })
                {
                    unsafe {
                        SendMessageW(self.tree.get(), TVM_EXPAND, TVE_EXPAND as usize, *item);
                    }
                }
            }
            if top.is_none() {
                restore_top = handles.first().copied().unwrap_or(0);
            }
            unsafe {
                if restore != 0 {
                    SendMessageW(
                        self.tree.get(),
                        TVM_SELECTITEM,
                        TVGN_CARET as usize,
                        restore,
                    );
                }
                // Restore the viewport after selection, which otherwise scrolls into view.
                if restore_top != 0 {
                    SendMessageW(
                        self.tree.get(),
                        TVM_SELECTITEM,
                        TVGN_FIRSTVISIBLE as usize,
                        restore_top,
                    );
                }
                SendMessageW(
                    self.tree.get(),
                    WM_HSCROLL,
                    SB_THUMBPOSITION as usize | ((horizontal as usize) << 16),
                    0,
                );
            }
        }
        *self.rendered.borrow_mut() = rows;
        drop(redraw);
        crate::diagnostics::event(
            "ui.tree.updated",
            serde_json::json!({"structural":structural,"rows":self.rendered.borrow().len()}),
        );
        self.rebuilding.set(false);
        self.details();
    }
    fn insert(
        &self,
        parent: HTREEITEM,
        text: &str,
        image: i32,
        selection: Option<Selection>,
    ) -> HTREEITEM {
        let mut text = wide(text);
        let insert = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
                    pszText: text.as_mut_ptr(),
                    iImage: image,
                    iSelectedImage: image,
                    ..Default::default()
                },
            },
        };
        let item = unsafe {
            SendMessageW(
                self.tree.get(),
                TVM_INSERTITEMW,
                0,
                (&insert as *const TVINSERTSTRUCTW) as isize,
            )
        };
        if let Some(selection) = selection {
            self.model.borrow_mut().items.insert(item, selection);
        }
        item
    }
    fn selected_item(&self, item: HTREEITEM) {
        if self.rebuilding.get() {
            return;
        }
        let selection = self.model.borrow().items.get(&item).cloned();
        crate::diagnostics::event("ui.selection", serde_json::json!({"selection":selection}));
        self.model.borrow_mut().selected = selection.clone();
        self.details();
        if let Some(selection) = selection {
            let _ = settings::save(&self.path.with_extension("ui.json"), &selection);
            if let Some((server, _, _)) = self.selected() {
                let path = self.path.clone();
                let generation = self.selection_generation.clone();
                let current = generation.fetch_add(1, Ordering::AcqRel) + 1;
                std::thread::spawn(move || {
                    let _ = settings::edit_client(&path, |config| {
                        if generation.load(Ordering::Acquire) == current {
                            config.select_server(server.saved);
                        }
                        Ok(())
                    });
                });
            }
        }
    }
    fn job(&self, label: &str, operation: impl FnOnce() -> io::Result<String> + Send + 'static) {
        if self.working.swap(true, Ordering::AcqRel) {
            return;
        }
        self.feedback.borrow_mut().set(label);
        self.details();
        let queue = self.events.clone();
        let working = self.working.clone();
        let window = self.window.get() as usize;
        std::thread::spawn(move || {
            let outcome = match operation() {
                Ok(message) => {
                    crate::diagnostics::event(
                        "ui.operation.complete",
                        serde_json::json!({"message":message}),
                    );
                    UiEvent::Notice(message)
                }
                Err(e) => {
                    crate::diagnostics::event(
                        "ui.operation.failed",
                        serde_json::json!({"error":e.to_string()}),
                    );
                    UiEvent::Error(e.to_string())
                }
            };
            queue.lock().unwrap().push(outcome);
            working.store(false, Ordering::Release);
            unsafe {
                PostMessageW(window as HWND, REFRESH, 0, 0);
            }
        });
    }
    fn command(&self, id: usize) {
        if self.working.load(Ordering::Acquire) && !matches!(id, ABOUT | HIDE) {
            return;
        }
        if let Some(language) = language_choice(id) {
            if let Err(error) = crate::tray::change_language(&self.path, Role::Client, language) {
                information(self.window.get(), &error.to_string());
            }
            return;
        }
        if matches!(id, CONNECT | DISCONNECT | PORTS | PROXY)
            && unsafe { GetMenuState(GetMenu(self.window.get()), id as u32, MF_BYCOMMAND) }
                & (MF_DISABLED | MF_GRAYED)
                != 0
        {
            return;
        }
        let selection = self.model.borrow().selected.clone();
        crate::diagnostics::event(
            "ui.action",
            serde_json::json!({"id":id,"action":match id{CONNECT=>"connect",DISCONNECT=>"disconnect",SEARCH=>"search",ADD_SERVER=>"add_server",PORTS=>"ports",PROXY=>"proxy",ABOUT=>"about",HIDE=>"hide",REMOVE_SERVER=>"remove_server",SELECT_SERVER=>"select_server",CAMERA_PREVIEW=>"camera_preview",DEVICE_CAMERA_PREVIEW=>"camera_test_device",_=>"other"},"selection":selection}),
        );
        let owner = self.window.get();
        match id {
            VIDEO_SETTINGS => video_settings(owner, &self.path, Role::Client),
            CAMERA_PREVIEW | DEVICE_CAMERA_PREVIEW => {
                let identity = (id == DEVICE_CAMERA_PREVIEW)
                    .then(|| self.selected().and_then(|(_, _, identity)| identity))
                    .flatten();
                if let Err(error) = crate::camera::launch(
                    &self.path,
                    Role::Client,
                    identity.as_deref(),
                    id == DEVICE_CAMERA_PREVIEW,
                ) {
                    information(owner, &error.to_string());
                }
            }
            SEARCH => self.request_refresh(),
            ABOUT => about(owner),
            HIDE => unsafe {
                ShowWindow(owner, SW_HIDE);
            },
            ADD_SERVER | PORTS => self.connection_form(id == ADD_SERVER),
            PROXY => self.proxy_form(),
            CONNECT => {
                if let Some((server, device, identity)) = self.selected() {
                    let path = self.path.clone();
                    if identity.is_none() {
                        self.job(crate::i18n::t("Connecting this server…"), move || {
                            agent::set_server_paused(&path, &server.saved, false)?;
                            Ok(crate::i18n::t("Server connection resumed.").into())
                        });
                    } else if let Some(device) = device {
                        self.job(
                            crate::i18n::t("Подключение устройства…"),
                            move || {
                                agent::remember_target(&path, &server.saved, &device)?;
                                Ok(
                                    crate::i18n::t("Устройство запомнено. Ожидание подключения…")
                                        .into(),
                                )
                            },
                        );
                    }
                }
            }
            DISCONNECT => {
                if let Some((server, _, identity)) = self.selected() {
                    let path = self.path.clone();
                    if let Some(identity) = identity {
                        self.job(
                            crate::i18n::t("Отключение устройства…"),
                            move || {
                                agent::release_target(&path, &server.saved, &identity)?;
                                Ok(crate::i18n::t(
                                    "Устройство отключено; автоматическое подключение выключено.",
                                )
                                .into())
                            },
                        );
                    } else {
                        self.job(crate::i18n::t("Disconnecting this server…"), move || {
                            agent::set_server_paused(&path, &server.saved, true)?;
                            Ok(crate::i18n::t(
                                "Server disconnected on this client. Device choices are preserved.",
                            )
                            .into())
                        });
                    }
                }
            }
            SELECT_SERVER => {
                if let Some((server, _, _)) = self.selected() {
                    let path = self.path.clone();
                    self.job(
                        crate::i18n::t("Сохранение выбранного сервера…"),
                        move || {
                            settings::edit_client(&path, |c| {
                                c.select_server(server.saved);
                                Ok(())
                            })?;
                            Ok(crate::i18n::t("Сервер выбран и сохранён.").into())
                        },
                    );
                }
            }
            REMOVE_SERVER => {
                if let Some((server, _, _)) = self.selected() {
                    let path = self.path.clone();
                    self.job(crate::i18n::t("Удаление сервера…"),move||{let config:ClientSettings=settings::read(&path)?;if let Some(saved)=config.connections().into_iter().find(|s|s.matches(&server.saved)){for target in saved.devices.iter().filter(|d|d.enabled){agent::release_target(&path,&saved,&target.identity)?;}}settings::edit_client(&path,|c|{let key=c.connections().iter().find(|s|s.matches(&server.saved)).map(SavedServer::key).unwrap_or(server.saved.key());c.remove_server(&key);Ok(())})?;Ok(crate::i18n::t("Сервер удалён из сохранённых. Если он доступен в сети, поиск сможет найти его снова.").into())});
                }
            }
            _ => {}
        }
    }
    fn proxy_form(&self) {
        let Some((server, _, _)) = self.selected() else {
            information(
                self.window.get(),
                crate::i18n::t("Выберите сервер в дереве устройств"),
            );
            return;
        };
        let previous = server.saved.proxy.clone();
        let existing = previous
            .as_ref()
            .and_then(|p| p.credential_file.as_ref())
            .and_then(|p| crate::credentials::load(p).ok());
        let fields = vec![
            Field {
                label: crate::i18n::t("Способ подключения").into(),
                value: match previous.as_ref().map(|p| &p.kind) {
                    None => 0,
                    Some(crate::proxy::Kind::Http) => 1,
                    Some(crate::proxy::Kind::Https) => 2,
                    Some(crate::proxy::Kind::Socks5) => 3,
                }
                .to_string(),
                kind: FieldKind::Choice(vec![
                    crate::i18n::t("Напрямую").into(),
                    "HTTP CONNECT".into(),
                    "HTTPS CONNECT (TLS)".into(),
                    "SOCKS5".into(),
                ]),
            },
            Field::text(
                crate::i18n::t("Адрес прокси: HOST:PORT"),
                previous
                    .as_ref()
                    .map(|p| p.address.clone())
                    .unwrap_or_default(),
            ),
            Field {
                label: crate::i18n::t("Авторизация").into(),
                value: usize::from(existing.is_some()).to_string(),
                kind: FieldKind::Choice(vec![
                    crate::i18n::t("Без авторизации").into(),
                    crate::i18n::t("Логин и пароль").into(),
                ]),
            },
            Field::text(
                crate::i18n::t("Логин"),
                existing
                    .as_ref()
                    .map(|c| c.username.clone())
                    .unwrap_or_default(),
            ),
            Field {
                label: crate::i18n::t("Пароль (пусто — сохранить прежний)").into(),
                value: String::new(),
                kind: FieldKind::Password,
            },
        ];
        let Some(values) = form_with_rules(
            self.window.get(),
            &crate::i18n::format(
                "Прокси — {} v.{}",
                &[
                    ("0", crate::APP_NAME.to_string()),
                    ("1", crate::APP_VERSION.to_string()),
                ],
            ),
            fields,
            false,
            FormRules::Proxy,
        ) else {
            return;
        };
        let result = (|| -> io::Result<Option<crate::proxy::Settings>> {
            let kind = match values[0].as_str() {
                "0" => return Ok(None),
                "1" => crate::proxy::Kind::Http,
                "2" => crate::proxy::Kind::Https,
                _ => crate::proxy::Kind::Socks5,
            };
            let credential_file = if values[2] == "1" {
                let password = if values[4].is_empty() {
                    existing
                        .as_ref()
                        .map(|c| c.password.clone())
                        .unwrap_or_default()
                } else {
                    values[4].clone()
                };
                if values[3].is_empty() || password.is_empty() {
                    return Err(io::Error::other(crate::i18n::t(
                        "Укажите логин и пароль прокси",
                    )));
                }
                Some(crate::credentials::save(&crate::proxy::Credentials {
                    username: values[3].clone(),
                    password,
                })?)
            } else {
                None
            };
            let proxy = crate::proxy::Settings {
                kind,
                address: settings::normalize_endpoint(values[1].trim(), 1080)?,
                credential_file,
            };
            proxy.validate()?;
            Ok(Some(proxy))
        })();
        match result {
            Err(error) => information(self.window.get(), &error.to_string()),
            Ok(proxy) => {
                let path = self.path.clone();
                self.job(crate::i18n::t("Сохранение прокси…"),move||{settings::edit_client(&path,|c|{
                    let saved = c.connections().into_iter().find(|s| s.matches(&server.saved)).unwrap_or(server.saved);
                    let key = saved.key();
                    if !c.connections().iter().any(|s| s.key() == key) { c.select_server(saved); }
                    if !c.update_connection(&key,|s|s.proxy=proxy) { return Err(io::Error::other(crate::i18n::t("Сервер больше не сохранён. Обновите список."))); }
                    Ok(())
                })?;agent::start_client(&path)?;Ok(crate::i18n::t("Настройки прокси сохранены. При изменении маршрута USB-подключения восстановятся автоматически.").into())});
            }
        }
    }
    fn connection_form(&self, add: bool) {
        let selected = if add {
            None
        } else {
            self.selected().map(|s| s.0.saved)
        };
        let config = self.model.borrow().config.clone();
        let (host, tcp) = selected
            .as_ref()
            .and_then(|s| settings::endpoint(&s.server).ok())
            .unwrap_or((String::new(), 3240));
        let fields = vec![
            Field::text(
                crate::i18n::t("Адрес сервера (IP или имя, без порта)"),
                host,
            ),
            Field::number(crate::i18n::t("TCP-порт сервера"), tcp),
            Field::number(
                crate::i18n::t("Порт поиска и списка устройств (UDP/TCP)"),
                config.discovery_port,
            ),
        ];
        let Some(values) = form(
            self.window.get(),
            &format!(
                "{} — {} v.{}",
                if add {
                    crate::i18n::t("Добавить сервер")
                } else {
                    crate::i18n::t("Порты подключения")
                },
                crate::APP_NAME,
                crate::APP_VERSION
            ),
            fields,
            false,
        ) else {
            return;
        };
        let result = (|| -> io::Result<(SavedServer, u16)> {
            let tcp = parse_port(&values[1])?;
            let udp = parse_port(&values[2])?;
            let host = values[0].trim().trim_matches(['[', ']']);
            let address = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                std::net::SocketAddr::new(ip, tcp).to_string()
            } else {
                if host.contains(':') {
                    return Err(io::Error::other(crate::i18n::t(
                        "Укажите адрес без порта; порт задаётся отдельным полем",
                    )));
                }
                format!("{host}:{tcp}")
            };
            settings::endpoint(&address)?;
            Ok((
                SavedServer {
                    paused: false,
                    proxy: None,
                    server: address,
                    server_id: selected.as_ref().and_then(|s| s.server_id.clone()),
                    name: selected
                        .as_ref()
                        .map(|s| s.name.clone())
                        .unwrap_or_default(),
                    devices: Vec::new(),
                },
                udp,
            ))
        })();
        match result {
            Err(e) => information(self.window.get(), &e.to_string()),
            Ok((mut server, udp)) => {
                let path = self.path.clone();
                self.job(
                    crate::i18n::t("Сохранение адреса и портов…"),
                    move || {
                        if let Ok(found) = agent::lookup_endpoint(&server.server, udp)
                            .and_then(|targets| {
                                discovery::find(udp, &targets, Duration::from_millis(650))
                            })
                            .and_then(|s| discovery::select(s, server.server_id.as_deref()))
                            && found.tcp_port_matches(&server.server)
                        {
                            server.server_id = Some(found.info.id);
                            server.name = found.info.name;
                        }
                        settings::edit_client(&path, |c| {
                            if let Some(key) = selected.as_ref().and_then(|selected| {
                                c.connections()
                                    .iter()
                                    .find(|s| s.matches(selected))
                                    .map(SavedServer::key)
                            }) {
                                c.update_connection(&key, |s| {
                                    s.server = server.server.clone();
                                    s.server_id = server.server_id.clone();
                                });
                            }
                            c.select_server(server);
                            c.discovery_port = udp;
                            Ok(())
                        })?;
                        Ok(crate::i18n::t("Адрес и порты сохранены.").into())
                    },
                );
            }
        }
    }
    fn popup(&self, lparam: LPARAM) {
        unsafe {
            let mut point: windows_sys::Win32::Foundation::POINT = std::mem::zeroed();
            let keyboard = lparam == -1;
            if keyboard {
                let item = SendMessageW(self.tree.get(), TVM_GETNEXTITEM, TVGN_CARET as usize, 0);
                let mut rect: RECT = std::mem::zeroed();
                *(&mut rect as *mut RECT as *mut HTREEITEM) = item;
                SendMessageW(
                    self.tree.get(),
                    TVM_GETITEMRECT,
                    1,
                    (&mut rect as *mut RECT) as isize,
                );
                point.x = rect.left;
                point.y = rect.bottom;
                ClientToScreen(self.tree.get(), &mut point);
            } else {
                point.x = (lparam as u32 & 0xffff) as i16 as i32;
                point.y = ((lparam as u32 >> 16) & 0xffff) as i16 as i32;
            }
            let mut hit = TVHITTESTINFO {
                pt: point,
                ..Default::default()
            };
            ScreenToClient(self.tree.get(), &mut hit.pt);
            SendMessageW(
                self.tree.get(),
                TVM_HITTEST,
                0,
                (&mut hit as *mut TVHITTESTINFO) as isize,
            );
            if !keyboard && hit.hItem != 0 {
                SendMessageW(
                    self.tree.get(),
                    TVM_SELECTITEM,
                    TVGN_CARET as usize,
                    hit.hItem,
                );
            }
            let menu = CreatePopupMenu();
            AppendMenuW(
                menu,
                MF_STRING | MF_DISABLED | MF_GRAYED,
                0,
                wide(&Role::Client.header()).as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            if let Some((_, device, identity)) = self.selected() {
                if identity.is_some() {
                    AppendMenuW(
                        menu,
                        MF_STRING
                            | if device.is_some() {
                                MF_ENABLED
                            } else {
                                MF_GRAYED
                            },
                        CONNECT,
                        wide(crate::i18n::t("Подключить и запомнить")).as_ptr(),
                    );
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        DISCONNECT,
                        wide(crate::i18n::t("Отключить устройство")).as_ptr(),
                    );
                } else {
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        CONNECT,
                        wide(crate::i18n::t("Connect server")).as_ptr(),
                    );
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        DISCONNECT,
                        wide(crate::i18n::t("Disconnect server")).as_ptr(),
                    );
                    for (id, text) in [
                        (SELECT_SERVER, crate::i18n::t("Выбрать сервер")),
                        (PORTS, crate::i18n::t("Адрес и порты…")),
                        (PROXY, crate::i18n::t("Прокси сервера…")),
                        (REMOVE_SERVER, crate::i18n::t("Удалить из сохранённых")),
                    ] {
                        AppendMenuW(menu, MF_STRING, id, wide(text).as_ptr());
                    }
                }
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
            }
            AppendMenuW(
                menu,
                MF_STRING,
                SEARCH,
                wide(crate::i18n::t("Найти серверы / обновить")).as_ptr(),
            );
            AppendMenuW(
                menu,
                MF_STRING,
                ADD_SERVER,
                wide(crate::i18n::t("Добавить сервер по адресу…")).as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            let camera_device = (keyboard || hit.hItem != 0)
                && self
                    .selected()
                    .is_some_and(|(_, _, identity)| identity.is_some());
            AppendMenuW(
                menu,
                MF_STRING | if camera_device { MF_ENABLED } else { MF_GRAYED },
                DEVICE_CAMERA_PREVIEW,
                wide(crate::i18n::t("Camera test…")).as_ptr(),
            );
            AppendMenuW(
                menu,
                MF_STRING,
                ABOUT,
                wide(crate::i18n::t("About")).as_ptr(),
            );
            for id in [
                CONNECT,
                DISCONNECT,
                SEARCH,
                ADD_SERVER,
                PORTS,
                PROXY,
                SELECT_SERVER,
                REMOVE_SERVER,
            ] {
                let state = GetMenuState(GetMenu(self.window.get()), id as u32, MF_BYCOMMAND);
                let disabled = self.working.load(Ordering::Acquire)
                    || (state != u32::MAX && state & (MF_GRAYED | MF_DISABLED) != 0);
                if disabled {
                    EnableMenuItem(menu, id as u32, MF_BYCOMMAND | MF_GRAYED);
                }
            }
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
    fn drain(&self) {
        let events = std::mem::take(&mut *self.events.lock().unwrap());
        let mut notice = None;
        let mut changed = false;
        for event in events {
            match event {
                UiEvent::Snapshot(snapshot) => {
                    let failed = snapshot
                        .servers
                        .iter()
                        .filter(|s| s.error.is_some())
                        .count();
                    self.feedback.borrow_mut().finish(
                        snapshot.ticket,
                        if failed == 0 {
                            crate::i18n::t("Поиск серверов и обновление устройств завершены").into()
                        } else {
                            crate::i18n::format(
                                "Обновление завершено. Недоступных серверов: {failed}",
                                &[("failed", format!("{failed}"))],
                            )
                        },
                    );
                    let mut model = self.model.borrow_mut();
                    model.servers = snapshot.servers;
                    model.found = snapshot.found;
                    model.config = snapshot.config;
                    model.report = snapshot.report;
                    let selected_key = model.selected.as_ref().map(|s| s.server.clone());
                    let upgraded = selected_key.and_then(|key| {
                        model
                            .servers
                            .iter()
                            .find(|s| s.saved.key() == key || s.addresses.contains(&key))
                            .map(|s| s.saved.key())
                    });
                    if let Some(key) = upgraded
                        && let Some(selection) = &mut model.selected
                    {
                        selection.server = key;
                    }
                    if model.selected.is_none() {
                        model.selected = model.config.connections().first().map(|s| Selection {
                            server: s.key(),
                            device: None,
                        });
                    }
                    if model.selected.as_ref().is_some_and(|selection| {
                        !model
                            .servers
                            .iter()
                            .any(|s| s.saved.key() == selection.server)
                    }) {
                        model.selected = model.config.connections().first().map(|s| Selection {
                            server: s.key(),
                            device: None,
                        });
                    }
                    changed = true;
                }
                UiEvent::Notice(message) => {
                    notice = Some(message);
                    self.refresh(false);
                }
                UiEvent::Error(message) => {
                    information(self.window.get(), &message);
                    notice = Some(message);
                }
                UiEvent::RefreshError(ticket, message) => {
                    self.feedback.borrow_mut().finish(
                        ticket,
                        crate::i18n::format(
                            "Ошибка обновления: {message}",
                            &[("message", message.to_string())],
                        ),
                    );
                }
            }
        }
        if let Some(message) = notice {
            self.feedback.borrow_mut().set(message);
        }
        if changed {
            self.render();
        } else {
            self.details();
        }
    }
}
fn phase_text(phase: &str) -> &str {
    match phase {
        "connected" => crate::i18n::t("Используется этим клиентом"),
        "connected_unconfirmed" => crate::i18n::t("Подключено; проверка сети"),
        "waiting_device" => crate::i18n::t("Ожидание возвращения USB"),
        "waiting_network" => crate::i18n::t("Ожидание сети"),
        "retrying" => crate::i18n::t("Повторное подключение"),
        "agent_unavailable" => crate::i18n::t("Фоновый агент недоступен"),
        _ => crate::i18n::t("Ожидание подключения"),
    }
}
unsafe extern "system" fn client_procedure(
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
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const ClientUi;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let context = unsafe { &*pointer };
    if let Some(result) = theme::message(window, message, wparam, lparam) {
        return result;
    }
    match message {
        CAMERA_MODAL => {
            context
                .modal_camera
                .begin(window, wparam as HWND, Role::Client);
            0
        }
        WM_CONTEXTMENU if wparam as HWND == context.tree.get() => {
            context.popup(lparam);
            0
        }
        WM_DPICHANGED => {
            context.fonts.borrow_mut().update(window);
            theme::dpi_rect(window, lparam);
            context.layout();
            0
        }
        WM_SIZE => {
            context.layout();
            0
        }
        WM_CLOSE => {
            crate::diagnostics::event(
                "ui.window.hide",
                serde_json::json!({"role":"client","reason":"close_button"}),
            );
            unsafe {
                ShowWindow(window, SW_HIDE);
            }
            0
        }
        WM_TIMER if wparam == 4 => {
            context.modal_camera.tick(window);
            0
        }
        WM_TIMER if wparam == 2 => {
            if unsafe { IsWindowVisible(window) } != 0 {
                context.refresh(false);
            }
            0
        }
        REFRESH => {
            context.drain();
            0
        }
        WM_COMMAND => {
            context.command(wparam & 0xffff);
            0
        }
        WM_NOTIFY => {
            let event = unsafe { &*(lparam as *const NMHDR) };
            if event.hwndFrom == context.tree.get() {
                if let Some(result) = theme::notification(lparam) {
                    return result;
                }
                match event.code {
                    TVN_SELCHANGEDW => {
                        let change = unsafe { &*(lparam as *const NMTREEVIEWW) };
                        context.selected_item(change.itemNew.hItem);
                    }
                    NM_DBLCLK => context.command(CONNECT),
                    _ => {}
                }
            }
            0
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            info.ptMinTrackSize.x = scale(window, 760);
            info.ptMinTrackSize.y = scale(window, 340);
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn video_settings_keep_an_absent_source_and_distinguish_equal_names() {
        let make = |id: &str| crate::camera::Camera {
            path: format!("path-{id}"),
            name: "Camera".into(),
            identities: vec![format!("USB\\VID_1234&PID_5678\\{id}")],
        };
        let cameras = [make("first"), make("second")];
        let saved = cameras[1].stable_identity();
        let (_, values, selected) = super::video_source_options(&cameras, &saved);
        assert_eq!(values[selected], saved);
        assert_eq!(selected, 2);
        let (_, values, selected) = super::video_source_options(&cameras[..1], &saved);
        assert_eq!(values[selected], saved);
        assert_ne!(values[selected], "test-pattern");
        let (_, values, selected) = super::video_source_options(&[], "test-pattern");
        assert_eq!(values[selected], "test-pattern");
    }
    #[test]
    fn port_fields_accept_only_the_full_valid_tcp_udp_range() {
        for value in ["1", "65535", " 3240 "] {
            assert!(super::parse_port(value).is_ok());
        }
        for value in [
            "",
            "0",
            "65536",
            "99999",
            "100000",
            "-1",
            "+1",
            "12.5",
            "１２３",
        ] {
            assert_eq!(
                super::parse_port(value).unwrap_err().to_string(),
                crate::i18n::t("Порт должен быть от 1 до 65535")
            );
        }
    }
    use super::*;
    #[test]
    fn modal_close_and_quit_release_menu_labels_and_window_context() {
        unsafe {
            let owner = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_OVERLAPPED,
                0,
                0,
                100,
                100,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!owner.is_null());
            let baseline = theme::menu_label_count();
            let title = "USBBridge memory fixture";
            let closer = std::thread::spawn(move || {
                for cycle in 0..33 {
                    let deadline = std::time::Instant::now() + Duration::from_secs(10);
                    loop {
                        let window =
                            FindWindowW(wide("USBBridge.Form.2").as_ptr(), wide(title).as_ptr());
                        if !window.is_null() {
                            if cycle == 32 {
                                let tid = GetWindowThreadProcessId(window, null_mut());
                                PostThreadMessageW(tid, WM_QUIT, 37, 0);
                            } else {
                                SendMessageW(window, WM_CLOSE, 0, 0);
                            }
                            break;
                        }
                        assert!(std::time::Instant::now() < deadline, "modal never appeared");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            });
            for _ in 0..33 {
                assert!(form(owner, title, Vec::new(), false).is_none());
                assert_eq!(
                    theme::menu_label_count(),
                    baseline,
                    "closed form retained menu strings"
                );
            }
            closer.join().unwrap();
            let mut message = std::mem::zeroed();
            assert_ne!(
                PeekMessageW(&mut message, null_mut(), WM_QUIT, WM_QUIT, PM_REMOVE),
                0
            );
            assert_eq!(message.wParam, 37);
            assert!(FindWindowW(wide("USBBridge.Form.2").as_ptr(), wide(title).as_ptr()).is_null());
            DestroyWindow(owner);
        }
    }
    #[test]
    fn stale_refresh_cannot_complete_a_later_action_or_erase_its_result() {
        let mut feedback = Feedback::default();
        feedback.begin(3, crate::i18n::t("Обновление устройств…"));
        feedback.finish(2, "old snapshot".into());
        assert_eq!(feedback.text, crate::i18n::t("Обновление устройств…"));
        assert_eq!(feedback.refresh, Some(3));
        feedback.finish(3, crate::i18n::t("Обновление устройств завершено").into());
        feedback.finish(4, "background snapshot".into());
        assert_eq!(
            feedback.text,
            crate::i18n::t("Обновление устройств завершено")
        );
        feedback.begin(5, "refresh");
        feedback.set(crate::i18n::t("Сохранение настроек…"));
        feedback.finish(5, "late refresh result".into());
        assert_eq!(feedback.text, crate::i18n::t("Сохранение настроек…"));
    }
    #[test]
    fn proxy_fields_follow_mode_and_authentication() {
        let enabled = |mode, auth| {
            (0..5)
                .map(|i| FormRules::Proxy.enabled(i, &[mode, -1, auth]))
                .collect::<Vec<_>>()
        };
        assert_eq!(enabled(0, 1), [true, false, false, false, false]);
        for mode in 1..=3 {
            assert_eq!(enabled(mode, 0), [true, true, true, false, false]);
            assert_eq!(enabled(mode, 1), [true, true, true, true, true]);
        }
    }
    #[test]
    fn device_rows_keep_identity_and_position_across_presence_publication_and_port_changes() {
        let device = crate::backend::mock_device();
        let saved = SavedServer {
            paused: false,
            server: "192.0.2.1:3240".into(),
            proxy: None,
            server_id: Some(uuid::Uuid::new_v4().to_string()),
            name: "server".into(),
            devices: Vec::new(),
        };
        let row = crate::host_status::DeviceRow {
            kind: crate::device_kind::DeviceKind::Camera,
            local_when_idle: false,
            identity: device.path.clone(),
            name: "Camera".into(),
            busid: device.busid.clone(),
            state: crate::i18n::t("Доступно").into(),
            present: true,
            published: true,
            client: "—".into(),
            tcp_port: "3240".into(),
        };
        let mut other = row.clone();
        other.identity = "other".into();
        other.name = "Bluetooth".into();
        other.published = false;
        let mut model = Model::default();
        model.servers.push(catalog::Server {
            saved: saved.clone(),
            addresses: vec![saved.server.clone()],
            devices: vec![device.clone()],
            error: None,
            catalog: Some(crate::management::Catalog {
                version: 1,
                server_id: saved.server_id.unwrap(),
                name: "server".into(),
                updated_unix_ms: 1,
                running: true,
                tcp_port: 3240,
                devices: vec![row, other],
            }),
        });
        let before = model.tree_rows();
        model.servers[0].devices.clear();
        let catalog = model.servers[0].catalog.as_mut().unwrap();
        catalog.updated_unix_ms = 999;
        catalog.devices.reverse();
        catalog.devices[1].present = false;
        catalog.devices[1].state = crate::i18n::t("Не подключено к USB").into();
        let absent = model.tree_rows();
        let keys = |rows: &[TreeRow]| {
            rows.iter()
                .map(|r| (r.parent, r.selection.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&before), keys(&absent));
        assert_ne!(before[3].text, absent[3].text);
        let catalog = model.servers[0].catalog.as_mut().unwrap();
        catalog.devices[1].present = true;
        catalog.devices[1].busid = "3-9".into();
        let moved = model.tree_rows();
        assert_eq!(keys(&before), keys(&moved));
        assert!(moved[3].text.contains("3-9"));
        model.servers[0].catalog.as_mut().unwrap().updated_unix_ms += 1;
        assert_eq!(moved, model.tree_rows());
    }
}
