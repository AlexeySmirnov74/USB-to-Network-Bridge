//! Read-only TCP catalog on the configured discovery port. The server enforces AllowedPeers.
use crate::{
    host_status::DeviceRow,
    settings::{self, SavedServer},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    time::Duration,
};
const REQUEST: &[u8] = b"USBBridge.Catalog.1\n";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    pub server_id: String,
    pub name: String,
    pub updated_unix_ms: u64,
    pub running: bool,
    pub tcp_port: u16,
    pub devices: Vec<DeviceRow>,
}
pub fn fetch(server: &SavedServer, port: u16) -> io::Result<Catalog> {
    let (host, _) = settings::endpoint(&server.server)?;
    let address = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut stream =
        crate::proxy::connect(&address, server.proxy.as_ref(), Duration::from_secs(3))?;
    stream
        .socket()
        .set_read_timeout(Some(Duration::from_secs(3)))?;
    stream
        .socket()
        .set_write_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(REQUEST)?;
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > 1024 * 1024 {
        return Err(io::Error::other("Server catalog exceeds 1 MiB"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    let catalog: Catalog = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if catalog.version != 1
        || server
            .server_id
            .as_ref()
            .is_some_and(|id| !id.eq_ignore_ascii_case(&catalog.server_id))
    {
        return Err(io::Error::other(
            "Server catalog identity does not match the saved server",
        ));
    }
    Ok(catalog)
}
#[cfg(windows)]
pub fn start(
    path: &std::path::Path,
    config: &settings::HostSettings,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> io::Result<std::thread::JoinHandle<()>> {
    use std::{
        net::TcpListener,
        sync::Arc,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };
    let listener = TcpListener::bind(("0.0.0.0", config.discovery_port))?;
    listener.set_nonblocking(true)?;
    let allowed = config.allowed_ips.clone();
    let file = path.with_extension("catalog.local.json");
    let pending = Arc::new(AtomicUsize::new(0));
    struct Pending(Arc<AtomicUsize>);
    impl Drop for Pending {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    thread::Builder::new().name("server-catalog".into()).spawn(move || {
        while !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut socket, peer)) => {
                    if !peer.ip().is_loopback() && !allowed.contains(&peer.ip()) {
                        crate::diagnostics::event("catalog.denied", serde_json::json!({"peer":peer.to_string()}));
                        continue;
                    }
                    if pending.load(Ordering::Acquire) >= 64 { continue; }
                    pending.fetch_add(1, Ordering::AcqRel);
                    let guard = Pending(pending.clone());
                    let path = file.clone();
                    let result = thread::Builder::new().name("catalog-request".into()).spawn(move || {
                        let _pending = guard;
                        let result = (|| -> io::Result<()> {
                            socket.set_read_timeout(Some(Duration::from_secs(2)))?;
                            socket.set_write_timeout(Some(Duration::from_secs(2)))?;
                            let mut request = vec![0; REQUEST.len()];
                            socket.read_exact(&mut request)?;
                            if request != REQUEST { return Err(io::Error::other("Unknown catalog request")); }
                            let value: Catalog = settings::read(&path)?;
                            let bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
                            if bytes.len() > 1024 * 1024 { return Err(io::Error::other("Catalog is too large")); }
                            socket.write_all(&(bytes.len() as u32).to_be_bytes())?;
                            socket.write_all(&bytes)
                        })();
                        crate::diagnostics::event("catalog.request", serde_json::json!({"peer":peer.to_string(),"error":result.err().map(|e|e.to_string())}));
                    });
                    if let Err(error) = result {
                        crate::diagnostics::event("catalog.worker.failed", serde_json::json!({"error":error.to_string()}));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    crate::diagnostics::event("catalog.listener.failed", serde_json::json!({"error":error.to_string()}));
                    break;
                }
            }
        }
    })
}
