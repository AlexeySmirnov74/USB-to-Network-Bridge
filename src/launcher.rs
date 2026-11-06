//! Double-click desktop entry points; setup and transport remain in their existing processes.
use crate::desktop::Role;
use std::{
    ffi::OsString,
    io,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
};

enum Action {
    Open(Option<PathBuf>),
    Help,
}
fn parse(args: Vec<OsString>) -> io::Result<Action> {
    match args.as_slice() {
        [] => Ok(Action::Open(None)),
        [help] if help == "--help" || help == "-h" => Ok(Action::Help),
        [option, path] if option == "--config" && !path.is_empty() => Ok(Action::Open(Some(
            std::path::absolute(PathBuf::from(path))?,
        ))),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage: double-click this EXE, or pass --config FILE. Use usbbridge.exe for command-line administration.",
        )),
    }
}
fn message(text: &str, error: bool) {
    let text = crate::desktop::wide(text);
    let title = crate::desktop::wide(crate::APP_NAME);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK
                | if error {
                    MB_ICONERROR
                } else {
                    MB_ICONINFORMATION
                },
        );
    }
}
fn launch(role: Role, action: Action) -> io::Result<bool> {
    let Action::Open(config) = action else {
        message(
            "Double-click to open this role's desktop interface. Portable copies use settings beside the EXE; MSI installations use their installed settings.\n\nOptional: --config FILE\nCommand-line administration: usbbridge.exe",
            false,
        );
        return Ok(true);
    };
    let runtime = std::env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::other("Application directory missing"))?
        .to_path_buf();
    let helper = runtime.join("Open-Installation.ps1");
    if !helper.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Keep this launcher with the other application files, including Open-Installation.ps1. Extract the complete portable ZIP or repair the MSI installation.",
        ));
    }
    let powershell =
        std::env::var_os("SystemRoot").ok_or_else(|| io::Error::other("SystemRoot is not set"))?;
    let mut command = Command::new(
        PathBuf::from(powershell).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    );
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(helper)
        .arg("-Role")
        .arg(if role == Role::Server {
            "Server"
        } else {
            "Client"
        })
        .arg("-Auto")
        .current_dir(runtime)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(config) = config {
        command.arg("-Config").arg(config);
    }
    // The helper reports setup errors in a dialog and exits after starting the existing GUI agent.
    Ok(command.status()?.success())
}
pub fn main(role: Role) {
    let result =
        parse(std::env::args_os().skip(1).collect()).and_then(|action| launch(role, action));
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            message(&error.to_string(), true);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_config_keeps_unicode_and_spaces() {
        let path = PathBuf::from(r"C:\USB folder\камера\client.local.json");
        let Action::Open(Some(actual)) =
            parse(vec!["--config".into(), path.clone().into_os_string()]).unwrap()
        else {
            panic!("expected an explicit configuration");
        };
        assert_eq!(actual, path);
    }
    #[test]
    fn unknown_or_incomplete_options_are_rejected() {
        for args in [
            vec!["--config".into()],
            vec!["--config".into(), "".into()],
            vec!["--role".into(), "server".into()],
            vec!["--config".into(), "a".into(), "--config".into(), "b".into()],
        ] {
            assert!(parse(args).is_err());
        }
    }
}
