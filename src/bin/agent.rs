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
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let result = if args.iter().any(|a| a == "--host") {
            usbbridge::agent::watch_host(&config, stop)
        } else if args.iter().any(|a| a == "--client") {
            usbbridge::agent::watch_client(&config, stop)
        } else {
            let role = args
                .windows(2)
                .find(|a| a[0] == "--role")
                .map(|a| {
                    if a[1].eq_ignore_ascii_case("server") {
                        usbbridge::desktop::Role::Server
                    } else {
                        usbbridge::desktop::Role::Client
                    }
                })
                .unwrap_or_else(|| usbbridge::desktop::Role::infer(&config));
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
