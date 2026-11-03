//! Background control plane. It never forwards USB payloads.
use crate::{
    discovery,
    recovery::{self, Driver, Port, Reconciler},
    settings::{self, ClientSettings, HostSettings},
};
use std::os::windows::process::CommandExt;
use std::{
    fs,
    io::{self, Write},
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
pub fn status_path(config: &Path) -> PathBuf {
    config.with_extension("status.json")
}
pub fn log(config: &Path, message: impl AsRef<str>) {
    crate::diagnostics::event(
        "controller.message",
        serde_json::json!({"message":message.as_ref()}),
    );
    let path = config.with_extension("agent.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
        let previous = config.with_extension("agent.previous.log");
        let _ = fs::remove_file(&previous);
        let _ = fs::rename(&path, previous);
    }
    if let Ok(mut file) = fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = writeln!(file, "{} {}", recovery::timestamp(), message.as_ref());
    }
}
pub fn start_client(config: &Path) -> io::Result<()> {
    let executable = std::env::current_exe()?.with_file_name("usbbridge-agent.exe");
    Command::new(executable)
        .arg("--client")
        .arg("--config")
        .arg(config)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
pub fn start_gui(config: &Path) -> io::Result<()> {
    Command::new(std::env::current_exe()?.with_file_name("usbbridge-agent.exe"))
        .args(["--gui", "--role", "client", "--config"])
        .arg(config)
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}
pub fn remember_target(
    path: &Path,
    server: &settings::SavedServer,
    device: &crate::wire::Device,
) -> io::Result<()> {
    settings::edit_client(path, |config| {
        config.select_server(server.clone());
        config.paused = false;
        config.remember(device, false);
        Ok(())
    })?;
    start_client(path)
}
pub fn release_target(
    path: &Path,
    server: &settings::SavedServer,
    identity: &str,
) -> io::Result<()> {
    let (server, target) = settings::edit_client(path, |config| {
        let server = config
            .connections()
            .into_iter()
            .find(|s| s.matches(server))
            .unwrap_or_else(|| server.clone());
        let mut target = None;
        config.update_connection(&server.key(), |s| {
            if let Some(device) = s
                .devices
                .iter_mut()
                .find(|d| d.identity.eq_ignore_ascii_case(identity))
            {
                device.enabled = false;
                target = Some(device.clone());
            }
        });
        Ok((server, target))
    })?;
    if let Some(target) = target {
        detach_target(path, &server, &target)?;
    }
    Ok(())
}
fn detach_target(
    path: &Path,
    server: &settings::SavedServer,
    target: &settings::Target,
) -> io::Result<()> {
    let identity = &target.identity;
    let _operation = settings::acquire_operation_lock(path, &server.key(), identity)?;
    let report = settings::read::<recovery::ClientReport>(&status_path(path)).ok();
    let actual = report
        .as_ref()
        .and_then(|r| r.servers.iter().find(|s| s.key == server.key()))
        .and_then(|r| {
            r.status
                .devices
                .iter()
                .find(|d| d.identity.eq_ignore_ascii_case(identity))
        });
    let busid = actual.map(|d| d.busid.as_str()).unwrap_or(&target.busid);
    let transport = crate::proxy_relay::routes(path)
        .into_iter()
        .find(|r| r.key == server.key())
        .map(|r| r.local)
        .unwrap_or_else(|| server.server.clone());
    crate::diagnostics::event(
        "device.release.request",
        serde_json::json!({"server":server.server,"identity":identity,"busid":busid,"transport":transport}),
    );
    crate::vhci::stop_attach_attempts(&transport, busid)?;
    for port in crate::vhci::imported_devices()?.iter().filter(|p| {
        p.server.eq_ignore_ascii_case(&transport)
            && (p.busid == busid || actual.is_some_and(|d| d.port == Some(p.port)))
    }) {
        crate::vhci::detach(port.port)?;
    }
    Ok(())
}
pub fn set_server_paused(
    path: &Path,
    server: &settings::SavedServer,
    paused: bool,
) -> io::Result<()> {
    let saved = settings::edit_client(path, |config| {
        let saved = config
            .connections()
            .into_iter()
            .find(|s| s.matches(server))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "server is no longer saved"))?;
        config.update_connection(&saved.key(), |s| s.paused = paused);
        Ok(saved)
    })?;
    crate::diagnostics::event(
        "server.connection.pause",
        serde_json::json!({"server":saved.server,"key":saved.key(),"paused":paused,"devices":saved.devices}),
    );
    if paused {
        // Commit the pause before waiting for any in-flight attach to finish. Its
        // operation lock then prevents a completed late attach escaping detach.
        let mut errors = Vec::new();
        for target in &saved.devices {
            if let Err(error) = detach_target(path, &saved, target) {
                errors.push(format!("{}: {error}", target.identity));
            }
        }
        if !errors.is_empty() {
            return Err(io::Error::other(errors.join("; ")));
        }
        Ok(())
    } else {
        start_client(path)
    }
}
struct WindowsDriver {
    path: PathBuf,
    key: String,
    listed: Vec<crate::wire::Device>,
    proxy: Option<crate::proxy::Settings>,
    relay: Option<crate::proxy_relay::Relay>,
    server: String,
}
impl WindowsDriver {
    fn transport<'a>(&'a self, server: &'a str) -> &'a str {
        self.relay
            .as_ref()
            .filter(|_| self.server == server)
            .map(|r| r.address.as_str())
            .unwrap_or(server)
    }
    fn configure(&mut self, config: &ClientSettings) -> io::Result<()> {
        if self.server == config.server
            && self.proxy == config.proxy
            && (config.proxy.is_none() || self.relay.is_some())
        {
            return Ok(());
        }
        if let Some(relay) = self.relay.take() {
            for port in crate::vhci::imported_devices()?
                .into_iter()
                .filter(|p| p.server == relay.address)
            {
                let _ = crate::vhci::stop_attach_attempts(&relay.address, &port.busid);
                let _ = crate::vhci::detach(port.port);
            }
            drop(relay);
            crate::proxy_relay::forget(&self.path, &self.key);
        }
        self.server = config.server.clone();
        self.proxy = config.proxy.clone();
        if let Some(proxy) = &config.proxy {
            let relay = crate::proxy_relay::Relay::start(config.server.clone(), proxy.clone())?;
            crate::proxy_relay::record(
                &self.path,
                &self.key,
                &config.server,
                &relay.address,
                &config
                    .devices
                    .iter()
                    .map(|d| d.busid.clone())
                    .collect::<Vec<_>>(),
            )?;
            self.relay = Some(relay);
        }
        Ok(())
    }
}
impl Drop for WindowsDriver {
    fn drop(&mut self) {
        if let Some(relay) = &self.relay {
            for route in crate::proxy_relay::routes(&self.path)
                .into_iter()
                .filter(|r| r.key == self.key)
            {
                for busid in route.busids {
                    let _ = crate::vhci::stop_attach_attempts(&relay.address, &busid);
                }
            }
            if let Ok(ports) = crate::vhci::imported_devices() {
                for port in ports.into_iter().filter(|p| p.server == relay.address) {
                    let _ = crate::vhci::detach(port.port);
                }
            }
            crate::proxy_relay::forget(&self.path, &self.key);
        }
    }
}
impl Driver for WindowsDriver {
    fn ports(&mut self) -> io::Result<Vec<Port>> {
        Ok(crate::vhci::imported_devices()?
            .into_iter()
            .map(|p| Port {
                number: p.port,
                server: if self.relay.as_ref().is_some_and(|r| r.address == p.server) {
                    self.server.clone()
                } else {
                    p.server
                },
                busid: p.busid,
            })
            .collect())
    }
    fn list(&mut self, server: &str) -> io::Result<Vec<crate::wire::Device>> {
        let devices =
            crate::client::list_with_proxy(server, self.proxy.as_ref(), Duration::from_secs(4))?;
        if self.listed != devices {
            crate::diagnostics::event(
                "usb.inventory.changed",
                serde_json::json!({"server":server,"devices":devices}),
            );
        }
        self.listed = devices.clone();
        Ok(devices)
    }
    fn attach(&mut self, server: &str, busid: &str, low_latency: bool) -> io::Result<i32> {
        let identity = self
            .listed
            .iter()
            .find(|d| d.busid == busid)
            .ok_or_else(|| io::Error::other("USB device disappeared"))?
            .path
            .clone();
        let _operation = settings::acquire_operation_lock(&self.path, &self.key, &identity)?;
        let latest: ClientSettings = settings::read(&self.path)?;
        let still_selected = latest.connections().iter().any(|s| {
            s.key() == self.key
                && !s.paused
                && s.proxy == self.proxy
                && s.server.eq_ignore_ascii_case(server)
                && s.devices.iter().any(|t| {
                    t.enabled
                        && self.listed.iter().any(|d| {
                            d.busid == busid
                                && d.path.eq_ignore_ascii_case(&t.identity)
                                && d.vendor == t.vendor
                                && d.product == t.product
                        })
                })
        });
        if !still_selected {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "USB selection changed",
            ));
        }
        // Cancel only this exact location's old kernel retry, never other devices.
        let transport = self.transport(server);
        if self.relay.is_some() {
            crate::proxy_relay::record(&self.path, &self.key, server, transport, &[busid.into()])?;
        }
        crate::diagnostics::event(
            "device.attach.begin",
            serde_json::json!({"server":server,"identity":identity,"busid":busid,"transport":transport}),
        );
        crate::vhci::stop_attach_attempts(transport, busid)?;
        let result = crate::vhci::attach(transport, busid, low_latency);
        crate::diagnostics::event(
            "device.attach.result",
            serde_json::json!({"server":server,"identity":identity,"busid":busid,"port":result.as_ref().ok(),"error":result.as_ref().err().map(ToString::to_string)}),
        );
        result
    }
    fn detach(&mut self, port: &Port) -> io::Result<()> {
        crate::diagnostics::event(
            "device.detach.begin",
            serde_json::json!({"server":port.server,"busid":port.busid,"port":port.number}),
        );
        crate::vhci::stop_attach_attempts(self.transport(&port.server), &port.busid)?;
        crate::vhci::detach(port.number)
    }
}
fn pause(stop: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && Instant::now() < end {
        thread::sleep(
            end.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        );
    }
}
pub fn watch_client(path: &Path, stop: Arc<AtomicBool>) -> io::Result<()> {
    crate::diagnostics::init(path, "client-controller");
    let _pnp = crate::pnp_diagnostics::Watch::start();
    let Some(_instance) = crate::instance::acquire("Client", "Control")? else {
        return Ok(());
    };
    let _single = match settings::acquire_file_lock(&path.with_extension("agent.lock")) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(32) => return Ok(()),
        Err(error) => return Err(error),
    };
    crate::proxy_relay::cleanup(path)?;
    let shared = Arc::new(Mutex::new(ClientSettings::default()));
    let reports = Arc::new(Mutex::new(std::collections::HashMap::<
        String,
        recovery::ConnectionStatus,
    >::new()));
    let mut workers =
        std::collections::HashMap::<String, (Arc<AtomicBool>, thread::JoinHandle<()>)>::new();
    log(path, "multi-server client recovery started");
    while !stop.load(Ordering::Acquire) {
        let loaded = settings::read::<ClientSettings>(path).and_then(|c| {
            c.validate()?;
            Ok(c)
        });
        match loaded {
            Ok(config) => {
                let connections = config.connections();
                *shared.lock().unwrap() = config.clone();
                let obsolete: Vec<_> = workers
                    .keys()
                    .filter(|key| !connections.iter().any(|s| s.key() == **key))
                    .cloned()
                    .collect();
                for key in obsolete {
                    if let Some((stop, worker)) = workers.remove(&key) {
                        stop.store(true, Ordering::Release);
                        let _ = worker.join();
                        reports.lock().unwrap().remove(&key);
                    }
                }
                for connection in &connections {
                    let key = connection.key();
                    workers.entry(key.clone()).or_insert_with(|| {
                        let finished = Arc::new(AtomicBool::new(false));
                        let worker_stop = finished.clone();
                        let worker_config = shared.clone();
                        let worker_reports = reports.clone();
                        let worker_path = path.to_path_buf();
                        let worker = thread::spawn(move || {
                            watch_connection(
                                &worker_path,
                                &key,
                                worker_config,
                                worker_reports,
                                worker_stop,
                            )
                        });
                        (finished, worker)
                    });
                }
                let snapshots = reports.lock().unwrap();
                let selected = connections
                    .first()
                    .and_then(|c| snapshots.get(&c.key()))
                    .map(|s| s.status.clone())
                    .unwrap_or_else(|| {
                        recovery::ClientStatus::error(&config.server, "Ожидание выбранного сервера")
                    });
                let servers = connections
                    .iter()
                    .filter_map(|c| snapshots.get(&c.key()).cloned())
                    .collect();
                let report = recovery::ClientReport { selected, servers };
                if let Err(error) = settings::save(&status_path(path), &report) {
                    log(path, format!("status write failed: {error}"));
                }
            }
            Err(error) => {
                let _ = settings::save(
                    &status_path(path),
                    &recovery::ClientStatus::error("", error),
                );
            }
        }
        pause(&stop, Duration::from_secs(1));
    }
    for (finished, _) in workers.values() {
        finished.store(true, Ordering::Release);
    }
    for (_, (_, worker)) in workers {
        let _ = worker.join();
    }
    log(
        path,
        "client recovery stopped; existing kernel USB sessions retained",
    );
    Ok(())
}
fn watch_connection(
    path: &Path,
    key: &str,
    shared: Arc<Mutex<ClientSettings>>,
    reports: Arc<Mutex<std::collections::HashMap<String, recovery::ConnectionStatus>>>,
    stop: Arc<AtomicBool>,
) {
    let mut reconciler = Reconciler::default();
    let mut driver = WindowsDriver {
        path: path.to_path_buf(),
        key: key.into(),
        listed: Vec::new(),
        proxy: None,
        relay: None,
        server: String::new(),
    };
    let started = Instant::now();
    let mut last = String::new();
    let mut discovery_due = Instant::now();
    let mut unreachable_since = None;
    let mut previous: Option<ClientSettings> = None;
    while !stop.load(Ordering::Acquire) {
        let config = {
            let all = shared.lock().unwrap();
            all.connections()
                .into_iter()
                .find(|c| c.key() == key)
                .map(|s| all.connection_config(&s))
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Server was removed"))
        };
        let (status, poll) = match config {
            Ok(mut config) => {
                if let Some(old) = &previous {
                    let retired: Vec<_> = old
                        .devices
                        .iter()
                        .filter(|d| {
                            d.enabled
                                && (old.server != config.server
                                    || old.proxy != config.proxy
                                    || !config.devices.iter().any(|t| {
                                        t.enabled && t.identity.eq_ignore_ascii_case(&d.identity)
                                    }))
                        })
                        .collect();
                    if !retired.is_empty()
                        && let Ok(ports) = driver.ports()
                    {
                        for target in retired {
                            let _ = crate::vhci::stop_attach_attempts(
                                driver.transport(&old.server),
                                &target.busid,
                            );
                            for port in ports.iter().filter(|p| {
                                p.server.eq_ignore_ascii_case(&old.server)
                                    && p.busid == target.busid
                            }) {
                                let _ = driver.detach(port);
                            }
                        }
                    }
                }
                previous = Some(config.clone());
                if config.server.is_empty() {
                    unreachable_since.get_or_insert_with(Instant::now);
                }
                if unreachable_since.is_some()
                    && config.discovery
                    && config.proxy.is_none()
                    && config.server_id.is_some()
                    && Instant::now() >= discovery_due
                {
                    discovery_due = Instant::now() + Duration::from_secs(15);
                    if let Ok(found) =
                        discovery::find(config.discovery_port, &[], Duration::from_secs(2))
                            .and_then(|s| discovery::select(s, config.server_id.as_deref()))
                        && found.address != config.server
                    {
                        // Re-read under the short edit lock so a user selection is not lost.
                        if let Ok(_edit) =
                            settings::acquire_file_lock(&path.with_extension("edit.lock"))
                            && let Ok(mut latest) = settings::read::<ClientSettings>(path)
                        {
                            let updated = latest.update_connection(key, |s| {
                                if s.server == config.server {
                                    s.server = found.address.clone();
                                }
                            });
                            if updated && settings::save(path, &latest).is_ok() {
                                config.server = found.address;
                            }
                        }
                    }
                }
                let status = if config.server.is_empty() {
                    recovery::ClientStatus::error("", "select a server and USB devices first")
                } else {
                    match driver.configure(&config) {
                        Ok(()) => reconciler.tick(&config, started.elapsed(), &mut driver),
                        Err(error) => recovery::ClientStatus::error(&config.server, error),
                    }
                };
                if status.devices.iter().any(|s| {
                    s.phase == "connected"
                        && config
                            .devices
                            .iter()
                            .any(|d| d.identity == s.identity && d.busid != s.busid)
                }) && let Ok(_edit) =
                    settings::acquire_file_lock(&path.with_extension("edit.lock"))
                    && let Ok(mut latest) = settings::read::<ClientSettings>(path)
                {
                    latest.update_connection(key, |s| {
                        if s.server != config.server {
                            return;
                        }
                        for device in &mut s.devices {
                            if let Some(current) = status
                                .devices
                                .iter()
                                .find(|s| s.phase == "connected" && s.identity == device.identity)
                            {
                                device.busid = current.busid.clone();
                            }
                        }
                    });
                    if let Err(error) = settings::save(path, &latest) {
                        log(path, format!("device location update failed: {error}"));
                    }
                }
                if status.error.is_some() {
                    unreachable_since.get_or_insert_with(Instant::now);
                } else {
                    unreachable_since = None;
                }
                (status, config.poll_seconds)
            }
            Err(error) => (recovery::ClientStatus::error("", error), 3),
        };
        let summary = format!(
            "{} {} {:?}",
            status.server,
            status.phase,
            status
                .devices
                .iter()
                .map(|d| (&d.busid, &d.phase, &d.error))
                .collect::<Vec<_>>()
        );
        if summary != last {
            log(path, &summary);
            crate::diagnostics::event(
                "recovery.state",
                serde_json::to_value(&status).unwrap_or_default(),
            );
            last = summary;
        }
        let name = previous
            .as_ref()
            .map(|s| s.server_name.clone())
            .unwrap_or_default();
        reports.lock().unwrap().insert(
            key.into(),
            recovery::ConnectionStatus {
                key: key.into(),
                name,
                status,
            },
        );
        pause(&stop, Duration::from_secs(poll));
    }
}
pub fn lookup_endpoint(server: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let (host, _) = settings::endpoint(server)?;
    Ok((host.as_str(), port).to_socket_addrs()?.collect())
}
fn switch_backend(config: &HostSettings, backend: &str, stop: bool) -> io::Result<()> {
    if !matches!(backend, "Native" | "Usbipd") {
        return Err(crate::wire::invalid("unknown backend state"));
    }
    let script = config.runtime.join("Switch-Backend.ps1");
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .arg("-Backend")
        .arg(backend)
        .arg("-RuntimeDirectory")
        .arg(&config.runtime)
        .arg("-Profile")
        .arg(&config.profile)
        .creation_flags(0x08000000);
    if stop {
        command.arg("-Stop");
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "backend recovery failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}
pub(crate) fn native_alive(config: &HostSettings, state: &serde_json::Value) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    };
    let Some(pid) = state["NativePid"]
        .as_u64()
        .and_then(|p| u32::try_from(p).ok())
    else {
        return false;
    };
    let Some(expected_time) = state["NativeCreatedFileTime"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return false;
    };
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited = created;
        let mut kernel = created;
        let mut user = created;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0
            && GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(process);
        let time = (created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64;
        ok && time == expected_time
            && String::from_utf16_lossy(&buffer[..length as usize]).eq_ignore_ascii_case(
                &config
                    .runtime
                    .join("usbbridge-server.exe")
                    .to_string_lossy(),
            )
    }
}
pub fn watch_host(path: &Path, stop: Arc<AtomicBool>) -> io::Result<()> {
    crate::diagnostics::init(path, "server-controller");
    let _pnp = crate::pnp_diagnostics::Watch::start();
    let Some(_instance) = crate::instance::acquire("Server", "Control")? else {
        return Ok(());
    };
    let _single = match settings::acquire_file_lock(&path.with_extension("agent.lock")) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(32) => return Ok(()),
        Err(error) => return Err(error),
    };
    let config: HostSettings = settings::read(path)?;
    config.validate()?;
    if let Ok(devices) = crate::inventory::load(&config.runtime.join("devices.inventory")) {
        for device in devices {
            crate::pnp_diagnostics::snapshot(&device.path);
        }
    }
    let _catalog = crate::management::start(path, &config, stop.clone())?;
    let mut last_catalog: Option<crate::management::Catalog> = None;
    let info = Arc::new(Mutex::new(discovery::ServerInfo {
        id: config.server_id.clone(),
        name: config.name.clone(),
        tcp_port: 0,
        backend: "Native".into(),
    }));
    let socket = UdpSocket::bind(("0.0.0.0", config.discovery_port))?;
    let _discovery = discovery::Responder::start(socket, info.clone(), config.allowed_ips.clone())?;
    let mut failures = 3u32;
    let mut next_recovery = Instant::now();
    let mut next_exports = Instant::now();
    let mut next_cleanup = Instant::now();
    let mut last = String::new();
    log(path, "host recovery and discovery started");
    while !stop.load(Ordering::Acquire) {
        let enabled = settings::read::<HostSettings>(path)
            .map(|c| c.enabled)
            .unwrap_or(false);
        let state = settings::read::<serde_json::Value>(&config.runtime.join("backend-state.json"));
        let backend = state
            .as_ref()
            .ok()
            .and_then(|s| {
                s["DesiredBackend"]
                    .as_str()
                    .or_else(|| s["Backend"].as_str())
            })
            .unwrap_or("Native");
        let expected_running = state.as_ref().is_ok_and(|s| {
            s["Backend"].as_str() == Some(backend)
                && (backend != "Native" || native_alive(&config, s))
        });
        let healthy = expected_running
            && crate::client::list_timeout(&config.probe_address, Duration::from_secs(3)).is_ok();
        let mut error = None;
        if !enabled {
            failures = 3;
            next_recovery = Instant::now();
            if (expected_running || crate::instance::stock_server_running()?)
                && let Err(failure) = switch_backend(&config, backend, true)
            {
                error = Some(failure.to_string());
            }
        } else if healthy {
            failures = 0;
            if Instant::now() >= next_exports {
                next_exports = Instant::now() + Duration::from_secs(3);
                match Command::new("powershell.exe")
                    .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                    .arg(config.runtime.join("Ensure-Exports.ps1"))
                    .arg("-RuntimeDirectory")
                    .arg(&config.runtime)
                    .arg("-Profile")
                    .arg(&config.profile)
                    .creation_flags(0x08000000)
                    .output()
                {
                    Ok(result) if result.status.success() => {}
                    Ok(result) => {
                        error = Some(format!(
                            "USB rebind: {}",
                            String::from_utf8_lossy(&result.stderr)
                        ));
                    }
                    Err(failure) => {
                        error = Some(failure.to_string());
                    }
                }
            }
        } else {
            failures = failures.saturating_add(1);
            if failures >= 3 && Instant::now() >= next_recovery {
                log(path, format!("restarting {backend} backend"));
                next_recovery = Instant::now() + Duration::from_secs(30);
                if let Err(failure) = switch_backend(&config, backend, false) {
                    error = Some(failure.to_string());
                    log(path, error.as_ref().unwrap());
                }
            }
        }
        let mut advertised = info.lock().unwrap();
        advertised.backend = backend.into();
        advertised.tcp_port = if healthy && enabled {
            settings::endpoint(&config.probe_address)?.1
        } else {
            0
        };
        drop(advertised);
        let phase = if !enabled {
            "stopped"
        } else if healthy {
            "running"
        } else {
            "recovering"
        };
        let summary = format!("{backend} {phase} {error:?}");
        if summary != last {
            log(path, &summary);
            last = summary;
        }
        if let Ok(dashboard) = crate::host_status::collect(path)
            && dashboard.presence_verified
        {
            let mut devices = dashboard.rows;
            let cleanup = settings::read::<HostSettings>(path).ok();
            let days = cleanup
                .as_ref()
                .filter(|c| c.auto_remove_unplugged)
                .map(|c| c.unplugged_retention_days);
            match crate::device_history::observe(
                path,
                &devices,
                dashboard.presence_verified,
                recovery::timestamp(),
                days,
            ) {
                Ok(expired) if Instant::now() >= next_cleanup => {
                    next_cleanup = Instant::now() + Duration::from_secs(60);
                    for identity in expired.into_iter().take(16) {
                        let result = crate::control::host_request(
                            path,
                            "Publish-Device.ps1",
                            &serde_json::json!({"identity":identity,"publish":false,"forget":true}),
                        );
                        crate::diagnostics::event(
                            "usb.cleanup",
                            serde_json::json!({"identity":identity,"days":days,"error":result.err().map(|e|e.to_string())}),
                        );
                    }
                }
                Ok(_) => {}
                Err(error) => log(path, format!("USB history: {error}")),
            }
            // Serialize with manual deletion: a refresh begun before deletion must not restore its row.
            let Ok(_history_lock) = crate::device_history::lock(path) else {
                pause(&stop, Duration::from_secs(1));
                continue;
            };
            let Ok(history) = crate::device_history::load(path) else {
                pause(&stop, Duration::from_secs(1));
                continue;
            };
            devices.retain(|row| history.visible(row));
            devices.sort_by(|a, b| a.name.cmp(&b.name).then(a.identity.cmp(&b.identity)));
            if last_catalog.as_ref().is_none_or(|c| c.devices != devices) {
                crate::diagnostics::event(
                    "usb.catalog.changed",
                    serde_json::json!({"devices":devices}),
                );
            }
            let catalog = crate::management::Catalog {
                version: 1,
                server_id: config.server_id.clone(),
                name: config.name.clone(),
                updated_unix_ms: recovery::timestamp(),
                running: dashboard.running,
                tcp_port: dashboard.tcp_port,
                devices,
            };
            let _ = settings::save(&path.with_extension("catalog.local.json"), &catalog);
            last_catalog = Some(catalog);
        }
        let status = serde_json::json!({"updated_unix_ms":recovery::timestamp(),"server":config.name,"server_id":config.server_id,"phase":phase,"backend":backend,"enabled":enabled,"error":error});
        let _ = settings::save(&status_path(path), &status);
        pause(&stop, Duration::from_secs(5));
    }
    Ok(())
}
