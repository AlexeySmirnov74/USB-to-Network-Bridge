//! Role-specific notification menu. USB recovery runs independently of the UI.
use crate::{
    agent,
    desktop::{self, ClientWindow, Role, ServerWindow, wide},
    settings,
};
use std::{
    cell::Cell,
    io,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
            Shell_NotifyIconW,
        },
        WindowsAndMessaging::*,
    },
};
struct Context {
    config: PathBuf,
    role: Role,
    icon: NOTIFYICONDATAW,
    taskbar_message: u32,
    client: Option<ClientWindow>,
    server: Option<ServerWindow>,
    reload: Cell<bool>,
}
fn notification_title(role: Role) -> &'static str {
    match role {
        Role::Server => "USBBridge.Notification.Server",
        Role::Client => "USBBridge.Notification.Client",
    }
}
pub fn control_interface(role: Role, close: bool) -> io::Result<bool> {
    unsafe {
        let (class, title) = if close {
            ("USBBridge.Notification.2", Some(notification_title(role)))
        } else {
            (
                match role {
                    Role::Server => "USBBridge.Server.2",
                    Role::Client => "USBBridge.Client.2",
                },
                None,
            )
        };
        let title = title.map(wide);
        let window = FindWindowW(
            wide(class).as_ptr(),
            title.as_ref().map_or(null(), |t| t.as_ptr()),
        );
        if window.is_null() {
            return Ok(false);
        }
        if PostMessageW(window, WM_CLOSE, 0, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
    }
}
pub fn change_language(
    config: &Path,
    role: Role,
    language: crate::i18n::Language,
) -> io::Result<()> {
    crate::i18n::save_language(config, language)?;
    crate::diagnostics::event(
        "ui.language.changed",
        serde_json::json!({"language":language.code(),"config":config}),
    );
    unsafe {
        let window = FindWindowW(
            wide("USBBridge.Notification.2").as_ptr(),
            wide(notification_title(role)).as_ptr(),
        );
        if !window.is_null() {
            PostMessageW(window, WM_APP + 21, 0, 0);
        }
    }
    Ok(())
}
fn can_reload(context: &Context) -> bool {
    context.client.as_ref().is_none_or(ClientWindow::can_reload)
        && context.server.as_ref().is_none_or(ServerWindow::can_reload)
}
fn activate_existing(role: Role) {
    unsafe {
        let class = wide("USBBridge.Notification.2");
        let title = wide(notification_title(role));
        for _ in 0..30 {
            let window = FindWindowW(class.as_ptr(), title.as_ptr());
            if !window.is_null() {
                PostMessageW(window, WM_APP + 20, 0, 0);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
fn status_text(config: &Path, role: Role) -> String {
    let Ok(status) = settings::read::<serde_json::Value>(&agent::status_path(config)) else {
        return crate::i18n::format(
            "{} — {}\nФоновое подключение ещё не запущено.",
            &[
                ("0", crate::APP_NAME.to_string()),
                ("1", role.label().to_string()),
            ],
        );
    };
    let phase = match status["phase"].as_str().unwrap_or("") {
        "running" => crate::i18n::t("Сервер работает"),
        "connected" => crate::i18n::t("Подключено"),
        "no_selection" => crate::i18n::t("Устройства не выбраны"),
        "paused" => crate::i18n::t("Server connection paused"),
        "recovering" => crate::i18n::t("Восстановление сервера"),
        "stopped" => crate::i18n::t("Сервер выключен"),
        _ => crate::i18n::t("Ожидание подключения"),
    };
    let mut result = format!(
        "{} — {}\n{phase}\n{}",
        crate::APP_NAME,
        role.header(),
        status["server"].as_str().unwrap_or("")
    );
    if role == Role::Server
        && let Ok(host) = settings::read::<settings::HostSettings>(config)
    {
        result.push_str(&format!(
            "\nTCP: {}   UDP: {}",
            settings::endpoint(&host.probe_address)
                .map(|a| a.1)
                .unwrap_or(3240),
            host.discovery_port
        ));
    }
    if let Some(servers) = status["servers"].as_array() {
        result.push_str(&crate::i18n::format(
            "\nСохранённых серверов: {}",
            &[("0", format!("{}", servers.len()))],
        ));
    }
    if let Some(error) = status["error"].as_str() {
        result.push_str(&format!("\n{error}"));
    }
    result
}
unsafe fn update(context: &Context, operation: u32) {
    let text = status_text(&context.config, context.role);
    let mut icon = context.icon;
    icon.szTip.fill(0);
    let tip = format!("{} — {}", crate::APP_NAME, context.role.header());
    for (slot, value) in icon.szTip[..127]
        .iter_mut()
        .zip(format!("{tip}\n{}", text.lines().nth(1).unwrap_or("")).encode_utf16())
    {
        *slot = value;
    }
    unsafe {
        Shell_NotifyIconW(operation, &icon);
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
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Context;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let context = unsafe { &*pointer };
    if let Some(result) = desktop::theme::message(window, message, wparam, lparam) {
        return result;
    }
    if message == context.taskbar_message {
        unsafe {
            update(context, NIM_ADD);
        }
        return 0;
    }
    match message {
        value if value == WM_APP + 20 => {
            if let Some(client) = &context.client {
                client.show(false);
            } else if let Some(server) = &context.server {
                server.show(false);
            }
            0
        }
        WM_TIMER => {
            if crate::i18n::preferred(&context.config) != crate::i18n::language()
                && can_reload(context)
            {
                context.reload.set(true);
                unsafe {
                    DestroyWindow(window);
                }
                return 0;
            }
            unsafe {
                update(context, NIM_MODIFY);
            }
            0
        }
        value if value == WM_APP + 21 => {
            if crate::i18n::preferred(&context.config) != crate::i18n::language()
                && can_reload(context)
            {
                context.reload.set(true);
                unsafe {
                    DestroyWindow(window);
                }
            }
            0
        }
        value if value == WM_APP + 1 => {
            let event = lparam as u32;
            if event == WM_LBUTTONDBLCLK {
                if let Some(client) = &context.client {
                    client.show(false);
                } else if let Some(server) = &context.server {
                    server.show(false);
                }
            } else if event == WM_RBUTTONUP {
                unsafe {
                    let menu = CreatePopupMenu();
                    AppendMenuW(
                        menu,
                        MF_STRING | MF_DISABLED | MF_GRAYED,
                        0,
                        wide(&context.role.header()).as_ptr(),
                    );
                    AppendMenuW(menu, MF_SEPARATOR, 0, null());
                    if context.role == Role::Client {
                        AppendMenuW(
                            menu,
                            MF_STRING,
                            1,
                            wide(crate::i18n::t("Открыть клиент")).as_ptr(),
                        );
                        AppendMenuW(
                            menu,
                            MF_STRING,
                            2,
                            wide(crate::i18n::t("Найти серверы")).as_ptr(),
                        );
                    } else {
                        AppendMenuW(
                            menu,
                            MF_STRING,
                            1,
                            wide(crate::i18n::t("Открыть сервер")).as_ptr(),
                        );
                    }
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        3,
                        wide(crate::i18n::t("Состояние подключения")).as_ptr(),
                    );
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        4,
                        wide(crate::i18n::t("Настройки и порты…")).as_ptr(),
                    );
                    AppendMenuW(menu, MF_SEPARATOR, 0, null());
                    AppendMenuW(menu, MF_STRING, 5, wide(crate::i18n::t("About")).as_ptr());
                    desktop::append_language_menu(menu);
                    AppendMenuW(menu, MF_SEPARATOR, 0, null());
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        6,
                        wide(crate::i18n::t("Закрыть интерфейс")).as_ptr(),
                    );
                    let mut point = std::mem::zeroed();
                    GetCursorPos(&mut point);
                    SetForegroundWindow(window);
                    desktop::theme::menu(menu, false);
                    let action = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        point.x,
                        point.y,
                        0,
                        window,
                        null(),
                    );
                    desktop::theme::destroy_menu(menu);
                    PostMessageW(window, WM_NULL, 0, 0);
                    crate::diagnostics::event(
                        "ui.tray.action",
                        serde_json::json!({"role":context.role.label(),"id":action}),
                    );
                    match action {
                        1 | 2 => {
                            if let Some(client) = &context.client {
                                client.show(action == 2);
                            } else if let Some(server) = &context.server {
                                server.show(false);
                            }
                        }
                        3 => {
                            if let Some(server) = &context.server {
                                server.show(false);
                            } else {
                                desktop::information(
                                    window,
                                    &status_text(&context.config, context.role),
                                );
                            }
                        }
                        4 => {
                            if let Some(client) = &context.client {
                                client.settings();
                            } else if let Some(server) = &context.server {
                                server.show(true);
                            }
                        }
                        5 => desktop::about(window),
                        6 => {
                            DestroyWindow(window);
                        }
                        id => {
                            if let Some(language) = desktop::language_choice(id as usize) {
                                if can_reload(context) {
                                    if let Err(error) =
                                        change_language(&context.config, context.role, language)
                                    {
                                        desktop::information(window, &error.to_string());
                                    }
                                } else {
                                    desktop::information(
                                        window,
                                        crate::i18n::t("Save settings before changing language."),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &context.icon);
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
pub fn run(config: &Path, role: Role, show: bool) -> io::Result<()> {
    if role == Role::Client
        && let Ok(settings) = settings::read::<settings::ClientSettings>(config)
        && settings.validate().is_ok()
        && settings
            .connections()
            .iter()
            .any(|s| !s.paused && s.devices.iter().any(|d| d.enabled))
        && let Err(error) = agent::start_client(config)
    {
        agent::log(
            config,
            format!("Could not start client supervision: {error}"),
        );
    }
    let role_name = if role == Role::Server {
        "Server"
    } else {
        "Client"
    };
    let Some(_instance) = crate::instance::acquire(role_name, "UI")? else {
        if show {
            activate_existing(role);
        }
        return Ok(());
    };
    let _single = match settings::acquire_file_lock(&config.with_extension("tray.lock")) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(32) => {
            if show {
                activate_existing(role);
            }
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut show = show;
    loop {
        crate::i18n::set_language(crate::i18n::preferred(config));
        if !run_window(config, role, show)? {
            break;
        }
        show = true;
    }
    Ok(())
}
fn run_window(config: &Path, role: Role, show: bool) -> io::Result<bool> {
    unsafe {
        let instance = GetModuleHandleW(null());
        let class_name = wide("USBBridge.Notification.2");
        let icon = LoadIconW(instance, 101usize as *const u16);
        if icon.is_null() {
            return Err(io::Error::last_os_error());
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            hIcon: icon,
            ..std::mem::zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut context = Box::new(Context {
            config: config.to_path_buf(),
            role,
            icon: std::mem::zeroed(),
            taskbar_message: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            reload: Cell::new(false),
            client: if role == Role::Client {
                Some(ClientWindow::create(config)?)
            } else {
                None
            },
            server: if role == Role::Server {
                Some(ServerWindow::create(config)?)
            } else {
                None
            },
        });
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide(notification_title(role)).as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            (&mut *context as *mut Context).cast(),
        );
        if window.is_null() {
            return Err(io::Error::last_os_error());
        }
        context.icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        context.icon.hWnd = window;
        context.icon.uID = 1;
        context.icon.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        context.icon.uCallbackMessage = WM_APP + 1;
        context.icon.hIcon = icon;
        update(&context, NIM_ADD);
        SetTimer(window, 1, 3000, None);
        if show {
            if let Some(client) = &context.client {
                client.show(false);
            } else if let Some(server) = &context.server {
                server.show(false);
            }
        }
        let mut message = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        let reload = context.reload.get();
        drop(context);
        UnregisterClassW(class_name.as_ptr(), instance);
        Ok(reload)
    }
}
