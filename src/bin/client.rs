use std::{
    io,
    path::{Path, PathBuf},
};
#[derive(Default)]
struct Options {
    config: Option<PathBuf>,
    server_id: Option<String>,
    json: bool,
    once: bool,
    no_agent: bool,
    discovery_port: Option<u16>,
}
fn parse_options() -> io::Result<(Vec<String>, Options)> {
    let mut positional = Vec::new();
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let missing = || io::Error::other(format!("{arg} requires a value"));
        match arg.as_str() {
            "--config" => options.config = Some(args.next().ok_or_else(missing)?.into()),
            "--server-id" => options.server_id = Some(args.next().ok_or_else(missing)?),
            "--discovery-port" => {
                options.discovery_port = Some(
                    args.next()
                        .ok_or_else(missing)?
                        .parse()
                        .map_err(io::Error::other)?,
                )
            }
            "--json" => options.json = true,
            "--once" => options.once = true,
            "--no-agent" => options.no_agent = true,
            _ => positional.push(arg),
        }
    }
    Ok((positional, options))
}
fn normalize_server(value: &str) -> io::Result<String> {
    usbbridge::settings::normalize_endpoint(value, 3240)
}
#[cfg(windows)]
fn managed_connect(
    server: &str,
    busid: &str,
    low_latency: bool,
    options: &Options,
    path: &Path,
) -> io::Result<()> {
    use usbbridge::{
        discovery,
        settings::{self, ClientSettings},
        vhci,
    };
    let _edit = settings::acquire_file_lock(&path.with_extension("edit.lock"))?;
    let mut config = match settings::read::<ClientSettings>(path) {
        Ok(config) => {
            config.validate()?;
            config
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => ClientSettings::default(),
        Err(error) => return Err(error),
    };
    let discovery_port = options.discovery_port.unwrap_or(config.discovery_port);
    let found = if server == "auto" {
        Some(discovery::select(
            discovery::find(discovery_port, &[], std::time::Duration::from_secs(2))?,
            options.server_id.as_deref(),
        )?)
    } else {
        let address = normalize_server(server)?;
        usbbridge::agent::lookup_endpoint(&address, discovery_port)
            .and_then(|targets| {
                discovery::find(
                    discovery_port,
                    &targets,
                    std::time::Duration::from_millis(650),
                )
            })
            .and_then(|servers| discovery::select(servers, options.server_id.as_deref()))
            .ok()
            .filter(|found| found.tcp_port_matches(&address))
    };
    let address = if server == "auto" {
        found
            .as_ref()
            .ok_or_else(|| io::Error::other("No server found"))?
            .address
            .clone()
    } else {
        normalize_server(server)?
    };
    let id = found
        .as_ref()
        .map(|s| s.info.id.clone())
        .or_else(|| options.server_id.clone());
    let proxy = config
        .connections()
        .into_iter()
        .find(|s| s.server.eq_ignore_ascii_case(&address))
        .and_then(|s| s.proxy);
    let devices = usbbridge::client::list_with_proxy(
        &address,
        proxy.as_ref(),
        std::time::Duration::from_secs(10),
    )?;
    let device = devices.iter().find(|d| d.busid == busid).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "USB device is not exported by this server",
        )
    })?;
    if proxy.is_some() {
        if options.once || options.no_agent {
            return Err(io::Error::other(
                "Proxy USB attachments require the recovery agent; omit --once/--no-agent",
            ));
        }
        config.select_server(settings::SavedServer {
            paused: false,
            server: address.clone(),
            proxy,
            server_id: id,
            name: found
                .as_ref()
                .map(|s| s.info.name.clone())
                .unwrap_or_default(),
            devices: Vec::new(),
        });
        config.remember(device, low_latency);
        config.validate()?;
        settings::save(path, &config)?;
        drop(_edit);
        usbbridge::agent::start_client(path)?;
        println!("Saved {busid} via proxy; the recovery agent will attach it automatically");
        return Ok(());
    }
    let port = if let Some(port) = vhci::imported_devices()?
        .into_iter()
        .find(|p| p.server.eq_ignore_ascii_case(&address) && p.busid == busid)
    {
        port.port
    } else {
        vhci::stop_attach_attempts(&address, busid)?;
        vhci::attach(&address, busid, low_latency)?
    };
    if !options.once {
        config.select_server(settings::SavedServer {
            paused: false,
            proxy: None,
            server: address.clone(),
            server_id: id,
            name: found
                .as_ref()
                .map(|s| s.info.name.clone())
                .unwrap_or_default(),
            devices: Vec::new(),
        });
        config.discovery_port = discovery_port;
        config.remember(device, low_latency);
        config.validate()?;
        settings::save(path, &config)?;
        if !options.no_agent {
            usbbridge::agent::start_client(path)?;
        }
    }
    println!(
        "Attached {busid} at virtual port {port}; server={address}; automatic_recovery={}",
        !options.once
    );
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("usbbridge-client: {e}");
        std::process::exit(1);
    }
}
fn run() -> io::Result<()> {
    let (args, options) = parse_options()?;
    let config_path = options
        .config
        .clone()
        .unwrap_or_else(usbbridge::settings::default_client_path);
    match args.first().map(String::as_str) {
        #[cfg(windows)]
        None | Some("gui") if args.len() <= 1 => usbbridge::agent::start_gui(&config_path),
        Some("discover") if args.len() == 1 => {
            let found = usbbridge::discovery::find(
                options.discovery_port.unwrap_or(usbbridge::discovery::PORT),
                &[],
                std::time::Duration::from_secs(2),
            )?;
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&found).map_err(io::Error::other)?
                );
            } else {
                for server in found {
                    println!(
                        "{} {} id={} backend={}",
                        server.info.name, server.address, server.info.id, server.info.backend
                    );
                }
            }
            Ok(())
        }
        Some("merge-inventory") if args.len() == 3 => {
            let existing = usbbridge::inventory::merge(
                usbbridge::inventory::load(Path::new(&args[1]))?,
                usbbridge::inventory::load(Path::new(&args[2]))?,
            );
            usbbridge::inventory::save(Path::new(&args[1]), &existing)
        }
        #[cfg(windows)]
        Some("forget-device-cache") if args.len() == 3 => {
            usbbridge::device_history::forget(Path::new(&args[1]), &args[2])
        }
        #[cfg(windows)]
        Some("capture-state") if args.len() == 2 => {
            use usbbridge::backend::Provider;
            let devices =
                usbbridge::vbox::NativeProvider::load(Path::new(&args[1]), false)?.devices()?;
            println!(
                "{}",
                serde_json::to_string(&devices).map_err(io::Error::other)?
            );
            Ok(())
        }
        #[cfg(windows)]
        Some("snapshot-local") if args.len() == 3 => {
            let device = usbbridge::usb_snapshot::snapshot(Path::new(&args[1]))?;
            usbbridge::inventory::save(Path::new(&args[2]), &[device])
        }
        Some("inventory") if args.len() == 2 => {
            let devices = usbbridge::inventory::load(Path::new(&args[1]))?;
            println!(
                "{}",
                serde_json::to_string(&devices).map_err(io::Error::other)?
            );
            Ok(())
        }
        Some("watch") if args.len() == 1 => {
            #[cfg(windows)]
            {
                usbbridge::agent::watch_client(
                    &config_path,
                    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                )
            }
            #[cfg(not(windows))]
            {
                Err(io::Error::other("watch requires Windows"))
            }
        }
        Some("status") if args.len() == 1 => {
            let mut status: usbbridge::recovery::ClientReport =
                usbbridge::settings::read(&config_path.with_extension("status.json"))?;
            status.invalidate_stale(usbbridge::recovery::timestamp());
            println!(
                "{}",
                serde_json::to_string_pretty(&status).map_err(io::Error::other)?
            );
            Ok(())
        }
        Some("ports") if args.len() == 1 => {
            #[cfg(windows)]
            {
                for device in usbbridge::vhci::imported_devices()? {
                    println!(
                        "port={} busid={} server={}",
                        device.port, device.busid, device.server
                    );
                }
                Ok(())
            }
            #[cfg(not(windows))]
            {
                Err(io::Error::other("ports requires Windows"))
            }
        }
        Some("snapshot") if args.len() >= 3 => {
            let mut devices = usbbridge::client::list(&args[1])?;
            if args.len() > 3 {
                let selected: std::collections::HashSet<_> = args[3..].iter().collect();
                devices.retain(|d| selected.contains(&d.busid));
                if devices.len() != selected.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "one or more selected devices are not shared by the stock server; bind them first",
                    ));
                }
            }
            usbbridge::inventory::save(std::path::Path::new(&args[2]), &devices)?;
            println!("Saved {} exported devices", devices.len());
            Ok(())
        }
        Some("install-driver") if args.len() == 2 => {
            #[cfg(windows)]
            {
                let id = usbbridge::vhci::install_root(std::path::Path::new(&args[1]))?;
                println!("{id}");
                Ok(())
            }
            #[cfg(not(windows))]
            {
                Err(io::Error::other("driver installation requires Windows"))
            }
        }
        Some("list") if args.len() == 2 => {
            let address = if args[1] == "auto" {
                usbbridge::discovery::select(
                    usbbridge::discovery::find(
                        options.discovery_port.unwrap_or(usbbridge::discovery::PORT),
                        &[],
                        std::time::Duration::from_secs(2),
                    )?,
                    options.server_id.as_deref(),
                )?
                .address
            } else {
                normalize_server(&args[1])?
            };
            let proxy =
                usbbridge::settings::read::<usbbridge::settings::ClientSettings>(&config_path)
                    .ok()
                    .and_then(|c| {
                        c.connections()
                            .into_iter()
                            .find(|s| s.server.eq_ignore_ascii_case(&address))
                    })
                    .and_then(|s| s.proxy);
            let devices = usbbridge::client::list_with_proxy(
                &address,
                proxy.as_ref(),
                std::time::Duration::from_secs(10),
            )?;
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&devices).map_err(io::Error::other)?
                );
                return Ok(());
            }
            for d in devices {
                println!(
                    "{} {:04x}:{:04x} speed={} interfaces={}",
                    d.busid,
                    d.vendor,
                    d.product,
                    d.speed,
                    d.interfaces.len()
                );
            }
            Ok(())
        }
        Some("bench") => {
            let server = args
                .get(1)
                .ok_or_else(|| io::Error::other("bench needs HOST:PORT"))?;
            let megabytes = args
                .get(2)
                .map_or(Ok(256), |v| v.parse::<u64>())
                .map_err(io::Error::other)?;
            let block = args
                .get(3)
                .map_or(Ok(262144), |v| v.parse::<usize>())
                .map_err(io::Error::other)?;
            let depth = args
                .get(4)
                .map_or(Ok(32), |v| v.parse::<usize>())
                .map_err(io::Error::other)?;
            let input = args.get(5).is_some_and(|v| v == "in");
            let result = usbbridge::client::bench(server, megabytes, block, depth, input)?;
            println!(
                "SYNTHETIC USB/IP benchmark {}: {} bytes, {:.3} s, {:.2} MiB/s, {:.3} Gbit/s",
                result.direction,
                result.bytes,
                result.seconds,
                result.mib_per_second,
                result.bytes as f64 * 8. / result.seconds / 1e9
            );
            Ok(())
        }
        Some("attach" | "connect")
            if args.len() == 3 || (args.len() == 4 && args[3] == "--low-latency") =>
        {
            #[cfg(windows)]
            {
                let low_latency = args.iter().any(|v| v == "--low-latency");
                managed_connect(&args[1], &args[2], low_latency, &options, &config_path)
            }
            #[cfg(not(windows))]
            {
                Err(io::Error::other(
                    "attach requires the Windows virtual USB driver",
                ))
            }
        }
        Some("detach") if args.len() == 2 => {
            #[cfg(windows)]
            {
                let number = args[1].parse().map_err(io::Error::other)?;
                if let Some(port) = usbbridge::vhci::imported_devices()?
                    .into_iter()
                    .find(|p| p.port == number)
                {
                    if let Ok(config) = usbbridge::settings::read::<
                        usbbridge::settings::ClientSettings,
                    >(&config_path)
                    {
                        let status = usbbridge::settings::read::<usbbridge::recovery::ClientReport>(
                            &config_path.with_extension("status.json"),
                        );
                        let routes = usbbridge::proxy_relay::routes(&config_path);
                        for server in config.connections().iter().filter(|s| {
                            s.server.eq_ignore_ascii_case(&port.server)
                                || routes.iter().any(|r| {
                                    r.key == s.key() && r.local.eq_ignore_ascii_case(&port.server)
                                })
                        }) {
                            if let Some(target) = server.devices.iter().find(|d| {
                                d.busid == port.busid
                                    || status.as_ref().is_ok_and(|r| {
                                        r.servers.iter().any(|s| {
                                            s.key == server.key()
                                                && s.status.devices.iter().any(|p| {
                                                    p.port == Some(number)
                                                        && p.identity
                                                            .eq_ignore_ascii_case(&d.identity)
                                                })
                                        })
                                    })
                            }) {
                                usbbridge::agent::release_target(
                                    &config_path,
                                    server,
                                    &target.identity,
                                )?;
                                println!("Detached port {}", args[1]);
                                return Ok(());
                            }
                        }
                    }
                    usbbridge::vhci::stop_attach_attempts(&port.server, &port.busid)?;
                }
                usbbridge::vhci::detach(number)?;
                println!("Detached port {}", args[1]);
                Ok(())
            }
            #[cfg(not(windows))]
            {
                Err(io::Error::other("detach requires Windows"))
            }
        }
        _ => {
            println!(
                "usbbridge-client discover [--json]\nusbbridge-client list HOST[:PORT]|auto [--server-id UUID] [--json]\nusbbridge-client connect HOST[:PORT]|auto BUSID [--config FILE] [--server-id UUID]\nusbbridge-client attach HOST[:PORT] BUSID [--low-latency] [--once] [--no-agent]\nusbbridge-client watch [--config FILE]\nusbbridge-client status [--config FILE]\nusbbridge-client ports\nusbbridge-client detach PORT [--config FILE]\nusbbridge-client snapshot HOST:PORT FILE [BUSID ...]\nusbbridge-client bench HOST:PORT [MiB=256] [block=262144] [depth=32] [in|out]\nSuccessful connect/attach remembers the device and starts recovery, unless --once.\nbench uses a synthetic device, not physical USB."
            );
            Ok(())
        }
    }
}
