use crate::{
    backend::Provider,
    tune_socket,
    wire::{self, Request},
};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

pub struct Server {
    provider: Arc<dyn Provider>,
    exports: HashSet<String>,
    all: bool,
    busy: Mutex<HashMap<String, Option<crate::host_status::Connection>>>,
    status_file: Option<PathBuf>,
    export_identities: Option<HashSet<String>>,
    allowed: Vec<IpAddr>,
    presence_interval: Duration,
}
impl Server {
    pub fn new(provider: Arc<dyn Provider>, exports: Vec<String>, allowed: Vec<IpAddr>) -> Self {
        let all = exports.iter().any(|s| s == "all");
        Self {
            provider,
            exports: exports.into_iter().collect(),
            all,
            busy: Mutex::new(HashMap::new()),
            status_file: None,
            export_identities: None,
            allowed,
            presence_interval: Duration::from_secs(2),
        }
    }
    pub fn with_presence_interval(mut self, interval: Duration) -> Self {
        self.presence_interval = interval.max(Duration::from_millis(10));
        self
    }
    pub fn with_status_file(mut self, path: PathBuf) -> Self {
        self.status_file = Some(path);
        self.publish(&self.busy.lock().unwrap());
        self
    }
    pub fn with_export_identities(mut self, identities: HashSet<String>) -> Self {
        self.export_identities = Some(identities);
        self
    }
    fn publish(&self, busy: &HashMap<String, Option<crate::host_status::Connection>>) {
        if let Some(path) = &self.status_file {
            let snapshot = crate::host_status::Sessions {
                process_id: std::process::id(),
                connections: busy.values().flatten().cloned().collect(),
            };
            if let Err(error) = crate::settings::save(path, &snapshot) {
                eprintln!("session status: {error}");
            }
        }
    }
    pub fn serve(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        for stream in listener.incoming() {
            let stream = stream?;
            let peer = stream.peer_addr()?;
            crate::diagnostics::event(
                "network.accept",
                serde_json::json!({"peer":peer,"local":stream.local_addr().ok()}),
            );
            if !peer.ip().is_loopback() && !self.allowed.contains(&peer.ip()) {
                eprintln!("denied peer {peer}");
                crate::diagnostics::event("network.denied", serde_json::json!({"peer":peer}));
                continue;
            }
            let server = self.clone();
            thread::Builder::new()
                .name(format!("usb-peer-{peer}"))
                .spawn(move || {
                    if let Err(e) = server.connection(stream)
                        && !matches!(
                            e.kind(),
                            io::ErrorKind::UnexpectedEof
                                | io::ErrorKind::ConnectionReset
                                | io::ErrorKind::BrokenPipe
                        )
                    {
                        eprintln!("{peer}: {e}");
                    }
                })?;
        }
        Ok(())
    }
    fn devices(&self) -> io::Result<Vec<wire::Device>> {
        Ok(self
            .provider
            .devices()?
            .into_iter()
            .filter(|d| {
                d.class != 9
                    && (self.all
                        || self
                            .export_identities
                            .as_ref()
                            .map(|ids| ids.contains(&d.path))
                            .unwrap_or_else(|| self.exports.contains(&d.busid)))
            })
            .collect())
    }
    pub fn connection(&self, mut stream: TcpStream) -> io::Result<()> {
        tune_socket(&stream)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let (op, status) = wire::read_operation(&mut stream)?;
        if status != 0 {
            return Err(wire::invalid("request operation status must be zero"));
        }
        match op {
            wire::DEVLIST => {
                let devices = self.devices()?;
                crate::diagnostics::event(
                    "usb.list",
                    serde_json::json!({"peer":stream.peer_addr().ok(),"devices":devices}),
                );
                let mut reply = Vec::with_capacity(12 + devices.len() * 316);
                reply.extend_from_slice(&wire::operation(5, 0));
                reply.extend_from_slice(&(devices.len() as u32).to_be_bytes());
                for d in devices {
                    reply.extend_from_slice(&d.encode()?);
                    for interface in d.interfaces {
                        reply.extend_from_slice(&interface);
                    }
                }
                stream.write_all(&reply)
            }
            wire::IMPORT => {
                let mut b = [0; 32];
                stream.read_exact(&mut b)?;
                let busid = wire::fixed_string(&b)?;
                crate::diagnostics::event(
                    "usb.import.request",
                    serde_json::json!({"busid":busid,"peer":stream.peer_addr().ok()}),
                );
                let Some(device) = self.devices()?.into_iter().find(|d| d.busid == busid) else {
                    return stream.write_all(&wire::operation(3, 1));
                };
                {
                    let mut busy = self.busy.lock().unwrap();
                    if busy.contains_key(&busid) {
                        return stream.write_all(&wire::operation(3, 1));
                    }
                    busy.insert(busid.clone(), None);
                }
                struct Lease<'a>(&'a Server, String);
                impl Drop for Lease<'_> {
                    fn drop(&mut self) {
                        let mut busy = self.0.busy.lock().unwrap();
                        busy.remove(&self.1);
                        self.0.publish(&busy);
                    }
                }
                let _lease = Lease(self, busid.clone());
                let (completed, rx) = mpsc::sync_channel(512);
                let session = match self.provider.open(&busid, completed) {
                    Ok(v) => v,
                    Err(e) => {
                        stream.write_all(&wire::operation(3, 1))?;
                        return Err(e);
                    }
                };
                stream.write_all(&wire::operation(3, 0))?;
                stream.write_all(&device.encode()?)?;
                {
                    let mut busy = self.busy.lock().unwrap();
                    busy.insert(
                        busid.clone(),
                        Some(crate::host_status::Connection {
                            identity: device.path.clone(),
                            busid: busid.clone(),
                            client: stream.peer_addr()?,
                            server: stream.local_addr()?,
                        }),
                    );
                    self.publish(&busy);
                }
                // This only polls cancellation. Idle/partial USB transfers are
                // retried indefinitely; there is no data-session expiry.
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                let disconnected = Arc::new(AtomicBool::new(false));
                let writer_stop = disconnected.clone();
                let mut output = stream.try_clone()?;
                let interrupt = stream.try_clone()?;
                let writer = thread::Builder::new()
                    .name(format!("usb-tx-{busid}"))
                    .spawn(move || {
                        for completion in rx {
                            if let Err(e) = completion.write(&mut output) {
                                writer_stop.store(true, Ordering::Release);
                                let _ = interrupt.shutdown(Shutdown::Both);
                                return Err(e);
                            }
                        }
                        let _ = output.shutdown(Shutdown::Write);
                        Ok(())
                    })?;
                eprintln!("attached {busid} from {}", stream.peer_addr()?);
                crate::diagnostics::event(
                    "usb.attached",
                    serde_json::json!({"busid":busid,"identity":device.path,"vendor":device.vendor,"product":device.product,"peer":stream.peer_addr().ok()}),
                );
                let presence = Presence::start(
                    self.provider.clone(),
                    device.path.clone(),
                    stream.try_clone()?,
                    self.presence_interval,
                    disconnected.clone(),
                    Arc::downgrade(&session),
                )?;
                let mut reader = SessionReader {
                    socket: &mut stream,
                    stop: disconnected.clone(),
                };
                let result = loop {
                    match wire::read_request(&mut reader) {
                        Ok(Request::Submit(req)) => {
                            if req.devid != device.devid() {
                                break Err(wire::invalid("device id mismatch"));
                            }
                            if let Err(e) = session.submit(req) {
                                break Err(e);
                            }
                        }
                        Ok(Request::Unlink { seq, target }) => {
                            if let Err(e) = session.cancel(seq, target) {
                                break Err(e);
                            }
                        }
                        Err(e) => break Err(e),
                    }
                };
                disconnected.store(true, Ordering::Release);
                let _ = stream.shutdown(Shutdown::Both);
                drop(presence);
                drop(session);
                let _ = writer.join();
                eprintln!("detached {busid}");
                crate::diagnostics::event(
                    "usb.detached",
                    serde_json::json!({"busid":busid,"identity":device.path,"peer":stream.peer_addr().ok(),"error":result.as_ref().err().map(ToString::to_string)}),
                );
                result
            }
            _ => Err(wire::invalid("unsupported management operation")),
        }
    }
}

struct Presence {
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Presence {
    fn start(
        provider: Arc<dyn Provider>,
        identity: String,
        socket: TcpStream,
        interval: Duration,
        disconnected: Arc<AtomicBool>,
        session: std::sync::Weak<dyn crate::backend::Session>,
    ) -> io::Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = stop.clone();
        let worker = thread::Builder::new()
            .name("usb-presence".into())
            .spawn(move || {
                let mut absent = 0;
                loop {
                    let (lock, wake) = &*signal;
                    let (done, _) = wake
                        .wait_timeout_while(lock.lock().unwrap(), interval, |done| !*done)
                        .unwrap();
                    if *done {
                        return;
                    }
                    drop(done);
                    // A failed presence query is not evidence of removal.
                    match provider.devices() {
                        Ok(devices)
                            if devices
                                .iter()
                                .any(|d| d.path.eq_ignore_ascii_case(&identity)) =>
                        {
                            absent = 0
                        }
                        Ok(_) => absent += 1,
                        Err(_) => continue,
                    }
                    // Even a quick unplug/replug can leave the old handle dead
                    // while an identically named interface already exists.
                    if absent >= 2 || session.upgrade().is_none_or(|s| !s.connected()) {
                        disconnected.store(true, Ordering::Release);
                        let _ = socket.shutdown(Shutdown::Both);
                        return;
                    }
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
struct SessionReader<'a> {
    socket: &'a mut TcpStream,
    stop: Arc<AtomicBool>,
}
impl Read for SessionReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            match self.socket.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                result => return result,
            }
        }
    }
}
impl Drop for Presence {
    fn drop(&mut self) {
        let (lock, wake) = &*self.stop;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
