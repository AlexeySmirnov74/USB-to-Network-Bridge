#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    {
        let args: Vec<_> = std::env::args().skip(1).collect();
        let config = args
            .windows(2)
            .find(|a| a[0] == "--config")
            .map(|a| std::path::PathBuf::from(&a[1]))
            .unwrap_or_else(usbbridge::settings::default_client_path);
        let role = if args.iter().any(|a| a == "--host") {
            usbbridge::desktop::Role::Server
        } else if args.iter().any(|a| a == "--client") {
            usbbridge::desktop::Role::Client
        } else {
            args.windows(2)
                .find(|a| a[0] == "--role")
                .map(|a| {
                    if a[1].eq_ignore_ascii_case("server") {
                        usbbridge::desktop::Role::Server
                    } else {
                        usbbridge::desktop::Role::Client
                    }
                })
                .unwrap_or_else(|| usbbridge::desktop::Role::infer(&config))
        };
        match usbbridge::lifecycle::is_exit_requested(&config, role) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                usbbridge::agent::log(&config, error.to_string());
                std::process::exit(1);
            }
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _parent = match usbbridge::supervisor::ParentWatch::start(&args, stop.clone()) {
            Ok(watch) => watch,
            Err(error) => {
                usbbridge::agent::log(&config, error.to_string());
                std::process::exit(1);
            }
        };
        let result = if args.iter().any(|a| a == "--check-supervisor") {
            usbbridge::supervisor::check(&config, role)
        } else if args.iter().any(|a| a == "--supervise") {
            usbbridge::supervisor::run(&config, role, stop)
        } else if args.iter().any(|a| a == "--host") {
            usbbridge::agent::watch_host(&config, stop)
        } else if args.iter().any(|a| a == "--client") {
            usbbridge::agent::watch_client(&config, stop)
        } else {
            if args.iter().any(|a| a == "--camera-preview") {
                let identity = args
                    .windows(2)
                    .find(|pair| pair[0] == "--camera-identity")
                    .map(|pair| pair[1].clone());
                let start = args.iter().any(|a| a == "--camera-autostart");
                let result = usbbridge::desktop::camera_preview(&config, role, identity, start);
                if let Err(error) = result {
                    usbbridge::agent::log(&config, error.to_string());
                    std::process::exit(1);
                }
                // This is the separate preview process, including any blocked third-party camera filter threads.
                std::process::exit(0);
            }
            usbbridge::diagnostics::init(
                &config,
                if role == usbbridge::desktop::Role::Server {
                    "server-gui"
                } else {
                    "client-gui"
                },
            );
            usbbridge::tray::run(&config, role, !args.iter().any(|a| a == "--tray"))
        };
        if let Err(error) = result {
            usbbridge::diagnostics::event(
                "process.exit",
                serde_json::json!({"exit_code":1,"error":error.to_string()}),
            );
            usbbridge::agent::log(&config, error.to_string());
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("USBBridge background agent requires Windows");
        std::process::exit(1);
    }
}
