//! Small, nonce-bound LAN discovery. Discovery never imports USB devices.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
pub const PORT: u16 = 3241;
const PROTOCOL: &str = "USBBridge.Discover.1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInfo {
    pub id: String,
    pub name: String,
    pub tcp_port: u16,
    pub backend: String,
}
impl ServerInfo {
    fn valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.id).is_ok()
            && !self.name.is_empty()
            && self.name.len() <= 128
            && self.tcp_port != 0
            && matches!(self.backend.as_str(), "Native" | "Usbipd" | "Mock")
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    protocol: String,
    nonce: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    protocol: String,
    nonce: String,
    server: ServerInfo,
}
#[derive(Clone, Debug, Serialize)]
pub struct FoundServer {
    pub address: String,
    /// All observed network interfaces of this single server identity.
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(flatten)]
    pub info: ServerInfo,
}
impl FoundServer {
    pub fn tcp_port_matches(&self, endpoint: &str) -> bool {
        crate::settings::endpoint(endpoint).is_ok_and(|(_, port)| port == self.info.tcp_port)
    }
}
pub struct Responder {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Responder {
    pub fn start(
        socket: UdpSocket,
        info: Arc<Mutex<ServerInfo>>,
        allowed: Vec<IpAddr>,
    ) -> io::Result<Self> {
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let finished = stop.clone();
        let worker = thread::Builder::new()
            .name("server-discovery".into())
            .spawn(move || {
                let mut buffer = [0u8; 1024];
                let mut last = std::collections::HashMap::<IpAddr, Instant>::new();
                while !finished.load(Ordering::Acquire) {
                    let (length, peer) = match socket.recv_from(&mut buffer) {
                        Ok(value) => value,
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock
                                    | io::ErrorKind::TimedOut
                                    | io::ErrorKind::Interrupted
                            ) =>
                        {
                            continue;
                        }
                        Err(_) => break,
                    };
                    if length > 256 || (!peer.ip().is_loopback() && !allowed.contains(&peer.ip())) {
                        continue;
                    }
                    let Ok(query) = serde_json::from_slice::<Query>(&buffer[..length]) else {
                        continue;
                    };
                    if query.protocol != PROTOCOL || uuid::Uuid::parse_str(&query.nonce).is_err() {
                        continue;
                    }
                    if last
                        .get(&peer.ip())
                        .is_some_and(|time| time.elapsed() < Duration::from_millis(100))
                    {
                        continue;
                    }
                    let server = info.lock().unwrap().clone();
                    if !server.valid() {
                        continue;
                    }
                    let reply = Reply {
                        protocol: PROTOCOL.into(),
                        nonce: query.nonce,
                        server,
                    };
                    if let Ok(data) = serde_json::to_vec(&reply) {
                        let _ = socket.send_to(&data, peer);
                        last.insert(peer.ip(), Instant::now());
                    }
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn broadcast(ip: Ipv4Addr, prefix: u8) -> Option<Ipv4Addr> {
    if prefix > 30 || ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Some(Ipv4Addr::from(u32::from(ip) | !mask))
}
#[cfg(windows)]
pub fn interfaces() -> io::Result<Vec<(Ipv4Addr, u8)>> {
    use windows_sys::Win32::{
        NetworkManagement::{
            IpHelper::{
                GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
                GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
            },
            Ndis::IfOperStatusUp,
        },
        Networking::WinSock::{AF_INET, SOCKADDR_IN},
    };
    let mut size = 16384u32;
    for _ in 0..4 {
        // usize storage supplies alignment required by Windows linked structs.
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let result = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                first,
                &mut size,
            )
        };
        if result == 111 && size <= 1024 * 1024 {
            continue;
        }
        if result == 232 {
            return Ok(Vec::new());
        }
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let mut output = Vec::new();
        let mut adapter = first;
        while !adapter.is_null() {
            let current = unsafe { &*adapter };
            if current.OperStatus == IfOperStatusUp {
                let mut address = current.FirstUnicastAddress;
                while !address.is_null() {
                    let current = unsafe { &*address };
                    if !current.Address.lpSockaddr.is_null()
                        && current.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                    {
                        let socket_address =
                            unsafe { &*current.Address.lpSockaddr.cast::<SOCKADDR_IN>() };
                        if socket_address.sin_family == AF_INET {
                            let raw = unsafe { socket_address.sin_addr.S_un.S_addr };
                            output.push((
                                Ipv4Addr::from(raw.to_ne_bytes()),
                                current.OnLinkPrefixLength,
                            ));
                        }
                    }
                    address = current.Next;
                }
            }
            adapter = current.Next;
        }
        output.sort();
        output.dedup();
        return Ok(output);
    }
    Err(io::Error::other(
        "network interfaces changed while enumerating",
    ))
}
#[cfg(not(windows))]
pub fn interfaces() -> io::Result<Vec<(Ipv4Addr, u8)>> {
    Ok(Vec::new())
}

pub fn find(port: u16, targets: &[SocketAddr], timeout: Duration) -> io::Result<Vec<FoundServer>> {
    let mut sockets = Vec::new();
    let mut destinations = Vec::new();
    if targets.is_empty() {
        for (ip, prefix) in interfaces()? {
            if let Some(broadcast) = broadcast(ip, prefix)
                && let Ok(socket) = UdpSocket::bind((ip, 0))
            {
                socket.set_broadcast(true)?;
                destinations.push(vec![SocketAddr::from((broadcast, port))]);
                sockets.push(socket);
            }
        }
        if sockets.is_empty() {
            let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
            socket.set_broadcast(true)?;
            sockets.push(socket);
            destinations.push(vec![SocketAddr::from((Ipv4Addr::BROADCAST, port))]);
        }
    } else {
        // IPv4 LAN discovery; direct USB/IP connections also support IPv6.
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        sockets.push(socket);
        destinations.push(
            targets
                .iter()
                .copied()
                .filter(SocketAddr::is_ipv4)
                .collect(),
        );
    }
    for socket in &sockets {
        socket.set_nonblocking(true)?;
    }
    let nonce = uuid::Uuid::new_v4().to_string();
    let query = serde_json::to_vec(&Query {
        protocol: PROTOCOL.into(),
        nonce: nonce.clone(),
    })
    .map_err(io::Error::other)?;
    let deadline = Instant::now() + timeout;
    let mut next_send = Instant::now();
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let mut buffer = [0u8; 2048];
    while Instant::now() < deadline {
        if Instant::now() >= next_send {
            for (socket, targets) in sockets.iter().zip(&destinations) {
                for address in targets {
                    let _ = socket.send_to(&query, address);
                }
            }
            next_send = Instant::now() + Duration::from_millis(500);
        }
        for socket in &sockets {
            for _ in 0..64 {
                match socket.recv_from(&mut buffer) {
                    Ok((length, peer)) => {
                        let Ok(reply) = serde_json::from_slice::<Reply>(&buffer[..length]) else {
                            continue;
                        };
                        if reply.protocol != PROTOCOL
                            || reply.nonce != nonce
                            || !reply.server.valid()
                        {
                            continue;
                        }
                        let address = SocketAddr::new(peer.ip(), reply.server.tcp_port).to_string();
                        if seen.insert((reply.server.id.clone(), address.clone())) {
                            results.push(FoundServer {
                                address,
                                addresses: Vec::new(),
                                info: reply.server,
                            });
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(group_servers(results))
}
fn address_rank(address: &str) -> (u8, String) {
    let rank = match address.parse::<SocketAddr>().map(|s| s.ip()) {
        Ok(IpAddr::V4(ip)) if ip.is_private() => 0,
        Ok(ip) if ip.is_loopback() => 2,
        _ => 1,
    };
    (rank, address.to_string())
}
pub fn group_servers(found: Vec<FoundServer>) -> Vec<FoundServer> {
    let mut grouped = std::collections::BTreeMap::<String, FoundServer>::new();
    for mut server in found {
        let key = server.info.id.to_ascii_lowercase();
        server.addresses.push(server.address.clone());
        if let Some(previous) = grouped.get_mut(&key) {
            previous.addresses.extend(server.addresses);
        } else {
            grouped.insert(key, server);
        }
    }
    grouped
        .into_values()
        .map(|mut server| {
            server.addresses.sort_by_key(|a| address_rank(a));
            server.addresses.dedup();
            server.address = server.addresses[0].clone();
            server
        })
        .collect()
}
pub fn select(mut found: Vec<FoundServer>, id: Option<&str>) -> io::Result<FoundServer> {
    if let Some(id) = id {
        found.retain(|s| s.info.id.eq_ignore_ascii_case(id));
    }
    let ids: HashSet<_> = found
        .iter()
        .map(|s| s.info.id.to_ascii_lowercase())
        .collect();
    if ids.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no unique matching server; specify its address or server ID",
        ));
    }
    // Prefer ordinary LAN addresses over a VPN when both identify this server.
    found.sort_by_key(|s| address_rank(&s.address));
    Ok(found.remove(0))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_network_interfaces_but_keep_distinct_servers_with_same_name() {
        let id = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let server = |id: String, address: &str| FoundServer {
            address: address.into(),
            addresses: Vec::new(),
            info: ServerInfo {
                id,
                name: "same host name".into(),
                tcp_port: 4100,
                backend: "Native".into(),
            },
        };
        let grouped = group_servers(vec![
            server(id.to_uppercase(), "26.0.0.2:4100"),
            server(id.clone(), "192.168.1.2:4100"),
            server(other.clone(), "192.168.1.3:4100"),
        ]);
        assert_eq!(grouped.len(), 2);
        let chosen = select(grouped.clone(), Some(&id)).unwrap();
        assert_eq!(chosen.address, "192.168.1.2:4100");
        assert_eq!(chosen.addresses.len(), 2);
        assert!(chosen.tcp_port_matches("192.168.1.2:4100"));
        assert!(!chosen.tcp_port_matches("192.168.1.2:49999"));
        assert!(select(grouped, None).is_err());
    }
    #[test]
    fn directed_broadcast_matches_subnet() {
        assert_eq!(
            broadcast(Ipv4Addr::new(192, 0, 2, 10), 24),
            Some(Ipv4Addr::new(192, 0, 2, 255))
        );
        assert_eq!(
            broadcast(Ipv4Addr::new(10, 9, 1, 3), 16),
            Some(Ipv4Addr::new(10, 9, 255, 255))
        );
        assert!(broadcast(Ipv4Addr::LOCALHOST, 8).is_none());
        assert!(broadcast(Ipv4Addr::new(192, 0, 2, 10), 32).is_none());
    }
    #[test]
    fn discovery_roundtrip_and_server_pinning() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let info = ServerInfo {
            id: id.clone(),
            name: "test host".into(),
            tcp_port: 3240,
            backend: "Mock".into(),
        };
        let _server = Responder::start(socket, Arc::new(Mutex::new(info)), Vec::new()).unwrap();
        let found = find(PORT, &[address], Duration::from_millis(150)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].address, "127.0.0.1:3240");
        assert!(select(found.clone(), Some(&uuid::Uuid::new_v4().to_string())).is_err());
        assert_eq!(select(found, Some(&id)).unwrap().info.id, id);
    }
}
