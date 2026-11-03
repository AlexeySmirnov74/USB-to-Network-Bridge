use std::{
    io,
    net::{IpAddr, SocketAddr, TcpListener},
    sync::Arc,
};
use usbbridge::{
    backend::{MockProvider, Provider},
    server::Server,
};
fn main() {
    if let Err(e) = run() {
        eprintln!("usbbridge-server: {e}");
        std::process::exit(1);
    }
}
fn run() -> io::Result<()> {
    let mut listen: SocketAddr = "127.0.0.1:3240".parse().unwrap();
    let mut exports = Vec::new();
    let mut allowed = Vec::<IpAddr>::new();
    let mut mock = false;
    let mut local_list = false;
    let mut backend = "native".to_string();
    let mut inventory = "devices.inventory".to_string();
    let mut debug = false;
    let mut session_status = None;
    let mut publication = None;
    let mut capture_helper = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--capture-helper" => {
                capture_helper =
                    Some(std::path::PathBuf::from(args.next().ok_or_else(|| {
                        io::Error::other("--capture-helper needs a script path")
                    })?))
            }
            "--publication" => {
                publication =
                    Some(std::path::PathBuf::from(args.next().ok_or_else(|| {
                        io::Error::other("--publication needs a file path")
                    })?))
            }
            "--session-status" => {
                session_status =
                    Some(std::path::PathBuf::from(args.next().ok_or_else(|| {
                        io::Error::other("--session-status needs a file path")
                    })?))
            }
            "--debug" => debug = true,
            "--backend" => {
                backend = args
                    .next()
                    .ok_or_else(|| io::Error::other("--backend needs native or usbdk"))?
            }
            "--inventory" => {
                inventory = args
                    .next()
                    .ok_or_else(|| io::Error::other("--inventory needs a file path"))?
            }
            "--install-capture-driver" | "--uninstall-capture-driver" => {
                #[cfg(windows)]
                {
                    let reboot = usbbridge::usbdk::install_capture_driver(
                        arg == "--uninstall-capture-driver",
                    )?;
                    println!("Capture driver operation complete; reboot_required={reboot}");
                    return Ok(());
                }
                #[cfg(not(windows))]
                {
                    return Err(io::Error::other("driver installation requires Windows"));
                }
            }
            "--listen" => {
                listen = args
                    .next()
                    .ok_or_else(|| io::Error::other("--listen needs an address"))?
                    .parse()
                    .map_err(io::Error::other)?
            }
            "--export" => exports.push(
                args.next()
                    .ok_or_else(|| io::Error::other("--export needs a busid or all"))?,
            ),
            "--allow-ip" => allowed.push(
                args.next()
                    .ok_or_else(|| io::Error::other("--allow-ip needs an IP"))?
                    .parse()
                    .map_err(io::Error::other)?,
            ),
            "--mock" => mock = true,
            "--list-local" => local_list = true,
            "-h" | "--help" => {
                println!(
                    "usbbridge-server [--backend native|usbdk] [--inventory FILE] [--listen IP:PORT]\n  [--export BUSID|all] [--allow-ip IP] [--session-status FILE] [--debug] [--list-local] [--mock]\nRepeat --export and --allow-ip as needed. No devices are exported by default.\nUse Switch-Backend.ps1 to select Native or the stock Usbipd service.\nNo trial timer or licensed-device count. --mock is a synthetic protocol test backend."
                );
                return Ok(());
            }
            _ => return Err(io::Error::other(format!("unknown argument {arg}"))),
        }
    }
    if !listen.ip().is_loopback() && allowed.is_empty() {
        return Err(io::Error::other(
            "specify at least one --allow-ip for a network listener",
        ));
    }
    usbbridge::diagnostics::init(
        &std::path::Path::new(&inventory).with_file_name("server.local.json"),
        "server-transport",
    );
    usbbridge::diagnostics::event(
        "server.start",
        serde_json::json!({"listen":listen,"backend":backend,"exports":exports}),
    );
    let provider: Arc<dyn Provider> = if mock {
        Arc::new(MockProvider)
    } else {
        #[cfg(windows)]
        {
            match backend.as_str() {
                "native" => {
                    let mut provider = usbbridge::vbox::NativeProvider::load(
                        std::path::Path::new(&inventory),
                        debug,
                    )?;
                    if let Some(path) = &publication {
                        provider = provider.with_publication(
                            Arc::new(usbbridge::publication::Store::load(path)?),
                            capture_helper.as_deref(),
                        )?;
                    }
                    Arc::new(provider)
                }
                "usbdk" => Arc::new(usbbridge::usbdk::UsbDkProvider::load()?),
                _ => return Err(io::Error::other("unknown backend: use native or usbdk")),
            }
        }
        #[cfg(not(windows))]
        {
            return Err(io::Error::other(
                "native capture currently targets Windows; use --mock for protocol tests",
            ));
        }
    };
    if local_list {
        for d in provider.devices()? {
            println!(
                "{} {:04x}:{:04x} speed={} interfaces={}",
                d.busid,
                d.vendor,
                d.product,
                d.speed,
                d.interfaces.len()
            );
        }
        return Ok(());
    }
    #[cfg(windows)]
    let _instance = if !mock {
        for _ in 0..20 {
            if !usbbridge::instance::stock_server_running()? {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if usbbridge::instance::stock_server_running()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "The stock USB/IP server is already running; use Switch-Backend.ps1 to change the active module",
            ));
        }
        Some(
            usbbridge::instance::acquire("Server", "Transport")?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "USB to Network Bridge server is already running",
                )
            })?,
        )
    } else {
        None
    };
    let listener = TcpListener::bind(listen)?;
    println!(
        "USBBridge listening on {}; backend={}; exports={:?}",
        listener.local_addr()?,
        if mock { "SYNTHETIC TEST" } else { &backend },
        exports
    );
    let identities = if !mock && backend == "native" && publication.is_none() {
        Some(
            usbbridge::inventory::load(std::path::Path::new(&inventory))?
                .into_iter()
                .filter(|d| exports.contains(&d.busid))
                .map(|d| d.path)
                .collect(),
        )
    } else {
        None
    };
    // With live publication the provider itself lists only the committed identities.
    // Static startup exports must not reject a device published afterwards.
    if publication.is_some() {
        if mock || backend != "native" {
            return Err(io::Error::other(
                "--publication requires the native backend",
            ));
        }
        exports = vec!["all".into()];
    }
    let mut server = Server::new(provider, exports, allowed);
    if let Some(identities) = identities {
        server = server.with_export_identities(identities);
    }
    if let Some(path) = session_status {
        server = server.with_status_file(path);
    }
    Arc::new(server).serve(listener)
}
