//! Explicit role exit. Windows startup remains enabled; this boot remembers
//! the user's choice so health tasks cannot undo it. The launcher resumes it.
use crate::{desktop::Role, settings, video};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::null,
    sync::OnceLock,
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject},
    UI::{
        Shell::{
            SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
            ShellExecuteExW,
        },
        WindowsAndMessaging::SW_HIDE,
    },
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExitState {
    version: u8,
    boot_identifier: [u8; 16],
    role: String,
}

fn role_name(role: Role) -> &'static str {
    if role == Role::Server {
        "server"
    } else {
        "client"
    }
}

fn boot_identifier() -> io::Result<[u8; 16]> {
    static BOOT: OnceLock<Result<[u8; 16], i32>> = OnceLock::new();
    let result = BOOT.get_or_init(|| {
        // SYSTEM_BOOT_ENVIRONMENT_INFORMATION begins with its BootIdentifier.
        // https://github.com/winsiderss/phnt/blob/master/ntexapi.h
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQuerySystemInformation(
                class: u32,
                information: *mut std::ffi::c_void,
                length: u32,
                returned: *mut u32,
            ) -> i32;
        }
        let mut information = [0u64; 8];
        let mut returned = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                90,
                information.as_mut_ptr().cast(),
                std::mem::size_of_val(&information) as u32,
                &mut returned,
            )
        };
        if status < 0 || returned < 16 {
            return Err(status);
        }
        let mut boot = [0u8; 16];
        boot[..8].copy_from_slice(&information[0].to_ne_bytes());
        boot[8..].copy_from_slice(&information[1].to_ne_bytes());
        if boot == [0; 16] { Err(-1) } else { Ok(boot) }
    });
    result.as_ref().copied().map_err(|status| {
        io::Error::other(format!(
            "Windows boot identity is unavailable (NTSTATUS {status:#x})"
        ))
    })
}

pub fn marker_path(config: &Path) -> PathBuf {
    config.with_extension("application-stop.local.json")
}

fn marker_paths(config: &Path) -> Vec<PathBuf> {
    let mut result = vec![marker_path(config)];
    for video in [
        video::receiver_path(config),
        video::sender_path(config),
        video::local_sender_path(config),
    ] {
        let marker = marker_path(&video);
        if video.is_file() || marker.is_file() {
            result.push(marker);
        }
    }
    result
}

fn held_for_boot(state: &ExitState, boot: &[u8; 16], role: Option<Role>) -> io::Result<bool> {
    if state.version != 1 || !matches!(state.role.as_str(), "server" | "client") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid application exit marker",
        ));
    }
    Ok(state.boot_identifier == *boot && role.is_none_or(|role| state.role == role_name(role)))
}

fn read_marker(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Application exit marker is too large",
        ));
    }
    Ok(Some(bytes))
}

fn requested(config: &Path, role: Option<Role>) -> io::Result<bool> {
    let Some(bytes) = read_marker(&marker_path(config))? else {
        return Ok(false);
    };
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let state: ExitState = serde_json::from_slice(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    held_for_boot(&state, &boot_identifier()?, role)
}

pub fn is_exit_requested(config: &Path, role: Role) -> io::Result<bool> {
    requested(config, Some(role))
}

pub fn video_exit_requested(config: &Path) -> io::Result<bool> {
    requested(config, None)
}

/// Internal coordinator command. Preflight and USB ownership are checked by
/// Quit-Application.ps1 before committing these sidecars.
pub fn mark_exit(config: &Path, role: Role) -> io::Result<()> {
    let state = ExitState {
        version: 1,
        boot_identifier: boot_identifier()?,
        role: role_name(role).into(),
    };
    let paths = marker_paths(config);
    let previous = paths
        .iter()
        .map(|path| read_marker(path).map(|bytes| (path.clone(), bytes)))
        .collect::<io::Result<Vec<_>>>()?;
    for path in &paths {
        if let Err(error) = settings::save(path, &state) {
            for (path, bytes) in &previous {
                if let Some(bytes) = bytes {
                    let _ = fs::write(path, bytes);
                } else {
                    let _ = fs::remove_file(path);
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

pub fn clear_exit(config: &Path) -> io::Result<()> {
    // Remove the main gate last, so a partial failure cannot restart the role.
    for path in marker_paths(config).into_iter().rev() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn quit(config: &Path, role: Role) -> io::Result<()> {
    invoke_helper(config, role, false)
}

pub fn resume(config: &Path, role: Role) -> io::Result<()> {
    if is_exit_requested(config, role)? {
        invoke_helper(config, role, true)?;
    }
    Ok(())
}

// Quote each native Windows argument, including empty strings and trailing
// backslashes. This is a process command line, never PowerShell source code.
fn helper_parameters(arguments: &[OsString]) -> io::Result<Vec<u16>> {
    let mut result = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            result.push(b' ' as u16);
        }
        result.push(b'"' as u16);
        let mut slashes = 0;
        for unit in argument.encode_wide() {
            match unit {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Application helper argument contains a NUL",
                    ));
                }
                92 => slashes += 1,
                34 => {
                    result.extend(std::iter::repeat_n(92, slashes * 2 + 1));
                    result.push(unit);
                    slashes = 0;
                }
                _ => {
                    result.extend(std::iter::repeat_n(92, slashes));
                    result.push(unit);
                    slashes = 0;
                }
            }
        }
        result.extend(std::iter::repeat_n(92, slashes * 2));
        result.push(b'"' as u16);
    }
    result.push(0);
    Ok(result)
}

fn elevated_helper(executable: &OsStr, arguments: &[OsString]) -> io::Result<u32> {
    let executable: Vec<u16> = executable.encode_wide().chain(Some(0)).collect();
    if executable[..executable.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Application helper path contains a NUL",
        ));
    }
    let parameters = helper_parameters(arguments)?;
    let verb: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
    let mut launch = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: verb.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        lpDirectory: null(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // runas presents normal Windows consent. FLAG_NO_UI only suppresses extra
    // Shell error dialogs; cancellation is returned to our existing UI worker.
    if unsafe { ShellExecuteExW(&mut launch) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if launch.hProcess.is_null() {
        return Err(io::Error::other(
            "Application helper process handle is missing",
        ));
    }
    // All paths close our retained process handle. No PID lookup, process kill,
    // timeout, or forced cancellation can interrupt USB cleanup/drain work.
    let process = unsafe { OwnedHandle::from_raw_handle(launch.hProcess) };
    match unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } {
        WAIT_OBJECT_0 => {}
        WAIT_FAILED => return Err(io::Error::last_os_error()),
        result => {
            return Err(io::Error::other(format!(
                "Unexpected application helper wait result: {result}"
            )));
        }
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(code)
}

fn invoke_helper(config: &Path, role: Role, resume: bool) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let runtime = executable
        .parent()
        .ok_or_else(|| io::Error::other("Application directory missing"))?;
    let helper = runtime.join("Quit-Application.ps1");
    if !helper.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Quit-Application.ps1 is missing; install the complete application folder",
        ));
    }
    let system = std::env::var_os("SystemRoot")
        .ok_or_else(|| io::Error::other("SystemRoot is unavailable"))?;
    let powershell = PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut arguments: Vec<OsString> = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    arguments.extend([
        helper.into_os_string(),
        "-Role".into(),
        if role == Role::Server {
            "Server"
        } else {
            "Client"
        }
        .into(),
        "-Config".into(),
        config.as_os_str().into(),
        "-RuntimeDirectory".into(),
        runtime.as_os_str().into(),
        "-Elevate".into(),
    ]);
    if resume {
        arguments.push("-Resume".into());
    }
    if !resume
        && executable.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .eq_ignore_ascii_case("usbbridge-agent.exe")
        })
    {
        arguments.extend([
            "-KeepProcessId".into(),
            std::process::id().to_string().into(),
        ]);
    }
    crate::diagnostics::event(
        "application.lifecycle.request",
        serde_json::json!({"role":role_name(role),"resume":resume,"launch":"runas"}),
    );
    let began = Instant::now();
    let code = match elevated_helper(powershell.as_os_str(), &arguments) {
        Ok(code) => code,
        Err(error) => {
            crate::diagnostics::event(
                "application.lifecycle.failed",
                serde_json::json!({"role":role_name(role),"resume":resume,"duration_ms":began.elapsed().as_millis(),"windows_error":error.raw_os_error(),"error":error.to_string()}),
            );
            return Err(error);
        }
    };
    if code != 0 {
        crate::diagnostics::event(
            "application.lifecycle.failed",
            serde_json::json!({"role":role_name(role),"resume":resume,"duration_ms":began.elapsed().as_millis(),"exit_code":code}),
        );
        return Err(io::Error::other(
            "Application stop/restart was canceled or failed; see the application lifecycle log",
        ));
    }
    crate::diagnostics::event(
        "application.lifecycle.complete",
        serde_json::json!({"role":role_name(role),"resume":resume,"duration_ms":began.elapsed().as_millis()}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt;

    #[test]
    fn helper_arguments_round_trip_through_the_windows_parser() {
        use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
        let mut arguments: Vec<OsString> = [
            "-File",
            r"C:\Program Files\USB Bridge\Quit-Application.ps1",
            "",
            r"C:\folder with spaces\",
            "embedded\"quote",
            "two\\\"slashes and quote",
            "$variable; & command | (test) `literal`",
            "Kamera/Камера/摄像头 📷",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        arguments.push(OsString::from_wide(&[0xd800, 92, 34, 92]));
        // CommandLineToArgvW treats argv[0] specially; prefix a real executable.
        let mut line: Vec<u16> = "powershell.exe ".encode_utf16().collect();
        line.extend(helper_parameters(&arguments).unwrap());
        let mut count = 0;
        let parsed = unsafe { CommandLineToArgvW(line.as_ptr(), &mut count) };
        assert!(!parsed.is_null());
        let observed = unsafe {
            let values = std::slice::from_raw_parts(parsed, count as usize);
            let result = values[1..]
                .iter()
                .map(|&value| {
                    let mut len = 0;
                    while *value.add(len) != 0 {
                        len += 1;
                    }
                    OsString::from_wide(std::slice::from_raw_parts(value, len))
                })
                .collect::<Vec<_>>();
            LocalFree(parsed.cast());
            result
        };
        assert_eq!(observed, arguments);
    }

    #[test]
    fn helper_arguments_reject_embedded_nuls() {
        let error =
            helper_parameters(&[OsString::from_wide(&[b'a' as u16, 0, b'b' as u16])]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn exit_is_role_specific_and_expires_on_the_next_boot() {
        let state = ExitState {
            version: 1,
            boot_identifier: [7; 16],
            role: "server".into(),
        };
        assert!(held_for_boot(&state, &[7; 16], Some(Role::Server)).unwrap());
        assert!(!held_for_boot(&state, &[7; 16], Some(Role::Client)).unwrap());
        assert!(!held_for_boot(&state, &[8; 16], Some(Role::Server)).unwrap());
        assert!(held_for_boot(&state, &[7; 16], None).unwrap());
    }
    #[test]
    fn marker_scope_covers_only_the_selected_role_and_its_video_files() {
        let root = std::env::temp_dir().join(format!("bridge-exit-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let host = root.join("host.local.json");
        let client = root.join("client.local.json");
        fs::write(&host, b"{}").unwrap();
        fs::write(&client, b"{}").unwrap();
        fs::write(video::receiver_path(&host), b"{}").unwrap();
        fs::write(video::receiver_path(&client), b"{}").unwrap();
        mark_exit(&host, Role::Server).unwrap();
        assert!(is_exit_requested(&host, Role::Server).unwrap());
        assert!(video_exit_requested(&video::receiver_path(&host)).unwrap());
        assert!(!is_exit_requested(&client, Role::Client).unwrap());
        assert!(!video_exit_requested(&video::receiver_path(&client)).unwrap());
        clear_exit(&host).unwrap();
        assert!(!is_exit_requested(&host, Role::Server).unwrap());
        assert!(!video_exit_requested(&video::receiver_path(&host)).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn oversized_or_invalid_markers_do_not_resume_workers() {
        let root =
            std::env::temp_dir().join(format!("bridge-exit-invalid-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let config = root.join("client.local.json");
        fs::write(marker_path(&config), vec![b' '; 4097]).unwrap();
        assert!(is_exit_requested(&config, Role::Client).is_err());
        fs::write(marker_path(&config), b"{}").unwrap();
        assert!(is_exit_requested(&config, Role::Client).is_err());
        clear_exit(&config).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
