//! Double-click desktop entry points; setup and transport remain in their existing processes.
use crate::desktop::Role;
use std::{
    ffi::OsString,
    io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
};

enum Action {
    Open(Option<PathBuf>),
    Help,
}

#[derive(Debug, PartialEq, Eq)]
struct LaunchLayout {
    install_root: PathBuf,
    runtime: PathBuf,
}

const ENTRY_POINTS: [&str; 2] = [
    "USB-to-Network-Bridge-Client.exe",
    "USB-to-Network-Bridge-Server.exe",
];

fn resolve_layout(executable: &Path) -> io::Result<LaunchLayout> {
    let directory = executable
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("Application directory missing"))?;
    let nested = directory.join("app");
    let (install_root, runtime) = if nested.try_exists()? {
        // An incomplete new layout must never fall back to stale flat helpers,
        // which could bypass the copied-installation identity decision.
        if !nested.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The application support path app must be a directory. Extract the complete package.",
            ));
        }
        (directory.to_path_buf(), nested)
    } else if directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("app"))
        && directory
            .parent()
            .is_some_and(|parent| ENTRY_POINTS.iter().all(|name| parent.join(name).is_file()))
    {
        // During migration a helper/old launcher may still run from app.
        // Recover the actual entry root only when both root launchers exist.
        (
            directory.parent().unwrap().to_path_buf(),
            directory.to_path_buf(),
        )
    } else {
        // Existing development and pre-migration MSI directories remain usable.
        (directory.to_path_buf(), directory.to_path_buf())
    };
    for name in [
        "Open-Installation.ps1",
        "FirstRun-Identity.ps1",
        "usbbridge-agent.exe",
    ] {
        if !runtime.join(name).is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "The complete application support files are required; {name} is missing. Keep both desktop EXEs with the app folder, or repair the MSI installation."
                ),
            ));
        }
    }
    Ok(LaunchLayout {
        install_root,
        runtime,
    })
}

fn prepare_command(
    role: Role,
    layout: &LaunchLayout,
    system_root: &Path,
    config: Option<PathBuf>,
) -> io::Result<Command> {
    let powershell = system_root.join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if !powershell.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Windows PowerShell is unavailable. The application setup could not be started.",
        ));
    }
    let mut command = Command::new(powershell);
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
        .arg(layout.runtime.join("Open-Installation.ps1"))
        .arg("-Role")
        .arg(if role == Role::Server {
            "Server"
        } else {
            "Client"
        })
        .arg("-Auto")
        .arg("-InstallRoot")
        .arg(&layout.install_root)
        .current_dir(&layout.runtime)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(config) = config {
        command.arg("-Config").arg(config);
    }
    Ok(command)
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
            "Double-click this role's EXE. Keep both desktop EXEs beside the complete app folder. Setup checks whether this folder belongs to this PC before opening the interface.\n\nOptional: --config FILE\nCommand-line administration: app\\usbbridge.exe (usbbridge.exe in a flat development directory).",
            false,
        );
        return Ok(true);
    };
    let layout = resolve_layout(&std::env::current_exe()?)?;
    let system_root =
        std::env::var_os("SystemRoot").ok_or_else(|| io::Error::other("SystemRoot is not set"))?;
    // Every launch, including an explicit/existing --config, first enters the
    // same copied-installation policy helper. Never start the GUI agent here.
    let mut command = prepare_command(role, &layout, &PathBuf::from(system_root), config)?;
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

    struct Fixture {
        root: PathBuf,
        temporary_parent: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let temporary_parent = std::env::temp_dir().canonicalize().unwrap();
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = temporary_parent.join(format!(
                "usbbridge-launcher-test-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir(&root).unwrap();
            Self {
                root,
                temporary_parent,
            }
        }
        fn file(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"fixture only; never execute").unwrap();
            path
        }
        fn payload(&self, prefix: &str) {
            for name in [
                "Open-Installation.ps1",
                "FirstRun-Identity.ps1",
                "usbbridge-agent.exe",
            ] {
                self.file(&format!("{prefix}{name}"));
            }
        }
        fn system_root(&self) -> PathBuf {
            self.file("Windows/System32/WindowsPowerShell/v1.0/powershell.exe");
            self.root.join("Windows")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Cleanup is restricted to the exact temporary directory created
            // above. Resolve before any recursive operation on Windows.
            if let Ok(resolved) = self.root.canonicalize()
                && resolved.parent() == Some(self.temporary_parent.as_path())
                && resolved
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("usbbridge-launcher-test-"))
            {
                let _ = std::fs::remove_dir_all(resolved);
            }
        }
    }

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

    #[test]
    fn app_directory_takes_precedence_over_stale_flat_helpers() {
        let fixture = Fixture::new();
        fixture.payload("");
        fixture.payload("app/");
        let executable = fixture.file(ENTRY_POINTS[0]);
        let layout = resolve_layout(&executable).unwrap();
        assert_eq!(layout.install_root, fixture.root);
        assert_eq!(layout.runtime, fixture.root.join("app"));
    }

    #[test]
    fn flat_development_and_existing_installations_remain_supported() {
        let fixture = Fixture::new();
        fixture.payload("");
        let layout = resolve_layout(&fixture.file(ENTRY_POINTS[1])).unwrap();
        assert_eq!(layout.runtime, fixture.root);
        assert_eq!(layout.install_root, fixture.root);
    }

    #[test]
    fn incomplete_app_never_uses_old_flat_identity_helpers() {
        let fixture = Fixture::new();
        fixture.payload("");
        fixture.file("app/usbbridge-agent.exe");
        let error = resolve_layout(&fixture.file(ENTRY_POINTS[0])).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn an_app_file_is_refused_instead_of_flat_fallback() {
        let fixture = Fixture::new();
        fixture.payload("");
        fixture.file("app");
        let error = resolve_layout(&fixture.file(ENTRY_POINTS[0])).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn missing_identity_helper_and_agent_are_refused_before_process_launch() {
        for missing in [
            "Open-Installation.ps1",
            "FirstRun-Identity.ps1",
            "usbbridge-agent.exe",
        ] {
            let fixture = Fixture::new();
            fixture.payload("app/");
            std::fs::remove_file(fixture.root.join("app").join(missing)).unwrap();
            assert!(resolve_layout(&fixture.file(ENTRY_POINTS[0])).is_err());
        }
    }

    #[test]
    fn an_executable_still_inside_app_recovers_the_root_during_migration() {
        let fixture = Fixture::new();
        fixture.payload("app/");
        for entry in ENTRY_POINTS {
            fixture.file(entry);
        }
        let executable = fixture.file("app/USB-to-Network-Bridge-Client.exe");
        let layout = resolve_layout(&executable).unwrap();
        assert_eq!(layout.install_root, fixture.root);
        assert_eq!(layout.runtime, fixture.root.join("app"));
    }

    #[test]
    fn an_unrelated_directory_named_app_keeps_its_flat_root() {
        let fixture = Fixture::new();
        fixture.payload("app/");
        let executable = fixture.file("app/USB-to-Network-Bridge-Client.exe");
        let layout = resolve_layout(&executable).unwrap();
        assert_eq!(layout.install_root, fixture.root.join("app"));
        assert_eq!(layout.runtime, fixture.root.join("app"));
    }

    #[test]
    fn explicit_existing_config_still_goes_through_auto_identity_policy() {
        let fixture = Fixture::new();
        fixture.payload("app/");
        let executable = fixture.file(ENTRY_POINTS[0]);
        let config = fixture.file("private settings/камера.local.json");
        let layout = resolve_layout(&executable).unwrap();
        let command = prepare_command(
            Role::Client,
            &layout,
            &fixture.system_root(),
            Some(config.clone()),
        )
        .unwrap();
        let args: Vec<OsString> = command.get_args().map(OsString::from).collect();
        assert!(args.iter().any(|arg| arg == "-Auto"));
        assert!(
            args.windows(2)
                .any(|pair| { pair[0] == "-InstallRoot" && pair[1] == fixture.root.as_os_str() })
        );
        assert!(args.windows(2).any(|pair| {
            pair[0] == "-File"
                && pair[1] == fixture.root.join("app/Open-Installation.ps1").as_os_str()
        }));
        assert!(
            args.windows(2)
                .any(|pair| { pair[0] == "-Config" && pair[1] == config.as_os_str() })
        );
        assert_eq!(command.get_current_dir(), Some(layout.runtime.as_path()));
        assert!(!args.iter().any(|arg| arg == "--gui"));
    }

    #[test]
    fn both_roles_use_only_role_auto_root_and_optional_config_arguments() {
        let fixture = Fixture::new();
        fixture.payload("app/");
        let layout = resolve_layout(&fixture.file(ENTRY_POINTS[0])).unwrap();
        for (role, name) in [(Role::Client, "Client"), (Role::Server, "Server")] {
            let command = prepare_command(role, &layout, &fixture.system_root(), None).unwrap();
            let args: Vec<OsString> = command.get_args().map(OsString::from).collect();
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "-Role" && pair[1] == name)
            );
            assert_eq!(
                args[8..],
                [
                    "-Role".into(),
                    name.into(),
                    "-Auto".into(),
                    "-InstallRoot".into(),
                    fixture.root.clone().into_os_string()
                ]
            );
        }
    }

    #[test]
    fn missing_powershell_is_reported_before_launching_setup() {
        let fixture = Fixture::new();
        fixture.payload("app/");
        let layout = resolve_layout(&fixture.file(ENTRY_POINTS[0])).unwrap();
        let error = prepare_command(Role::Client, &layout, &fixture.root.join("missing"), None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
