//! Loopback adapter for the signed driver's direct WSK sockets, used only for proxy routes.
use crate::{diagnostics, proxy};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Route {
    pub key: String,
    pub server: String,
    pub local: String,
    pub busids: Vec<String>,
}
static ROUTES: Mutex<()> = Mutex::new(());
pub fn routes(config: &std::path::Path) -> Vec<Route> {
    crate::settings::read(&config.with_extension("proxy-routes.local.json")).unwrap_or_default()
}
pub fn record(
    config: &std::path::Path,
    key: &str,
    server: &str,
    local: &str,
    busids: &[String],
) -> io::Result<()> {
    let _lock = ROUTES.lock().unwrap();
    let mut values = routes(config);
    if let Some(route) = values.iter_mut().find(|r| r.key == key) {
        route.local = local.into();
        route.server = server.into();
        for busid in busids {
            if !route.busids.contains(busid) {
                route.busids.push(busid.clone());
            }
        }
    } else {
        values.push(Route {
            key: key.into(),
            server: server.into(),
            local: local.into(),
            busids: busids.to_vec(),
        });
    }
    crate::settings::save(&config.with_extension("proxy-routes.local.json"), &values)
}
pub fn forget(config: &std::path::Path, key: &str) {
    let _lock = ROUTES.lock().unwrap();
    let mut values = routes(config);
    values.retain(|r| r.key != key);
    let _ = crate::settings::save(&config.with_extension("proxy-routes.local.json"), &values);
}
pub fn cleanup(config: &std::path::Path) -> io::Result<()> {
    for route in routes(config) {
        let address: std::net::SocketAddr = route.local.parse().map_err(io::Error::other)?;
        if address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
            return Err(io::Error::other("Invalid saved proxy relay address"));
        }
        for busid in &route.busids {
            crate::vhci::stop_attach_attempts(&route.local, busid)?;
        }
        for port in crate::vhci::imported_devices()?
            .into_iter()
            .filter(|p| p.server == route.local && route.busids.contains(&p.busid))
        {
            crate::vhci::detach(port.port)?;
        }
        forget(config, &route.key);
    }
    Ok(())
}
pub struct Relay {
    pub address: String,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<HashMap<u64, Vec<TcpStream>>>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Relay {
    pub fn start(target: String, settings: proxy::Settings) -> io::Result<Self> {
        settings.validate()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(HashMap::<u64, Vec<TcpStream>>::new()));
        let finished = stop.clone();
        let registered = sockets.clone();
        diagnostics::event(
            "proxy.relay.start",
            serde_json::json!({"local":address,"server":target,"kind":settings.kind,"proxy":settings.address}),
        );
        let worker = thread::Builder::new()
            .name("proxy-listener".into())
            .spawn(move || {
                let sequence = AtomicU64::new(0);
                while !finished.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((local, _)) => {
                            let id = sequence.fetch_add(1, Ordering::Relaxed);
                            let target = target.clone();
                            let proxy = settings.clone();
                            let stop = finished.clone();
                            let sockets = registered.clone();
                            if let Ok(copy) = local.try_clone() {
                                sockets.lock().unwrap().insert(id, vec![copy]);
                            }
                            let cleanup = registered.clone();
                            if let Err(error) = thread::Builder::new()
                                .name("proxy-connection".into())
                                .spawn(move || {
                                    serve_connection(local, id, target, proxy, stop, sockets)
                                })
                            {
                                cleanup.lock().unwrap().remove(&id);
                                diagnostics::event(
                                    "proxy.relay.worker.failed",
                                    serde_json::json!({"error":error.to_string()}),
                                );
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(25))
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            address,
            stop,
            sockets,
            worker: Some(worker),
        })
    }
}
fn serve_connection(
    mut local: TcpStream,
    id: u64,
    target: String,
    proxy: proxy::Settings,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<HashMap<u64, Vec<TcpStream>>>>,
) {
    let start = Instant::now();
    let result = (|| {
        if stop.load(Ordering::Acquire) {
            return Ok((0, 0));
        }
        crate::tune_socket(&local)?;
        let mut remote = proxy::connect(&target, Some(&proxy), Duration::from_secs(10))?;
        if let Ok(copy) = remote.socket().try_clone()
            && let Some(values) = sockets.lock().unwrap().get_mut(&id)
        {
            values.push(copy);
        }
        pump(&mut local, &mut remote, &stop)
    })();
    if let Some(values) = sockets.lock().unwrap().remove(&id) {
        for socket in values {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
    match result {
        Ok((sent, received)) => diagnostics::event(
            "proxy.relay.closed",
            serde_json::json!({"server":target,"bytes_to_server":sent,"bytes_to_client":received,"elapsed_ms":start.elapsed().as_millis() as u64}),
        ),
        Err(error) => diagnostics::event(
            "proxy.relay.error",
            serde_json::json!({"server":target,"error":error.to_string()}),
        ),
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Finish accepting before taking the shutdown snapshot: an accepted
        // socket must not be registered after we have already closed the others.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        for values in self.sockets.lock().unwrap().values() {
            for socket in values {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        diagnostics::event(
            "proxy.relay.stop",
            serde_json::json!({"local":self.address}),
        );
    }
}
struct Buffer {
    bytes: Vec<u8>,
    offset: usize,
    length: usize,
}
impl Buffer {
    fn new() -> Self {
        Self {
            bytes: vec![0; 256 * 1024],
            offset: 0,
            length: 0,
        }
    }
    fn empty(&self) -> bool {
        self.offset == self.length
    }
    fn receive(&mut self, source: &mut impl Read) -> io::Result<Option<usize>> {
        if !self.empty() {
            return Ok(None);
        }
        self.offset = 0;
        self.length = 0;
        match source.read(&mut self.bytes) {
            Ok(n) => {
                self.length = n;
                Ok(Some(n))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
    fn send(&mut self, target: &mut impl Write) -> io::Result<usize> {
        if self.empty() {
            return Ok(0);
        }
        match target.write(&self.bytes[self.offset..self.length]) {
            Ok(0) => Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                self.offset += n;
                Ok(n)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }
}
pub fn pump(
    local: &mut TcpStream,
    remote: &mut proxy::Stream,
    stop: &AtomicBool,
) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::*;
    local.set_nonblocking(true)?;
    remote.socket().set_read_timeout(None)?;
    remote.socket().set_write_timeout(None)?;
    remote.socket().set_nonblocking(true)?;
    let mut up = Buffer::new();
    let mut down = Buffer::new();
    let (mut sent, mut received) = (0, 0);
    let (mut local_eof, mut remote_eof) = (false, false);
    while !stop.load(Ordering::Acquire) {
        let mut progress = false;
        let n = up.send(remote)?;
        sent += n as u64;
        progress |= n > 0;
        let n = down.send(local)?;
        received += n as u64;
        progress |= n > 0;
        if !local_eof && let Some(n) = up.receive(local)? {
            local_eof = n == 0;
            progress = true;
        }
        if !remote_eof && let Some(n) = down.receive(remote)? {
            remote_eof = n == 0;
            progress = true;
        }
        if (local_eof || remote_eof) && up.empty() && down.empty() {
            break;
        }
        if !progress {
            let mut sockets = [
                WSAPOLLFD {
                    fd: local.as_raw_socket() as usize,
                    events: if up.empty() && !local_eof {
                        POLLRDNORM
                    } else {
                        0
                    } | if !down.empty() { POLLWRNORM } else { 0 },
                    revents: 0,
                },
                WSAPOLLFD {
                    fd: remote.socket().as_raw_socket() as usize,
                    events: if down.empty() && !remote_eof {
                        POLLRDNORM
                    } else {
                        0
                    } | if !up.empty() { POLLWRNORM } else { 0 },
                    revents: 0,
                },
            ];
            if unsafe { WSAPoll(sockets.as_mut_ptr(), 2, 200) } == SOCKET_ERROR {
                return Err(io::Error::last_os_error());
            }
        }
    }
    remote.shutdown();
    let _ = local.shutdown(Shutdown::Both);
    Ok((sent, received))
}
