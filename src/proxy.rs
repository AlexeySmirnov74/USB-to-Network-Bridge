//! Explicit proxy routes. SOCKS names resolve at the proxy; HTTP uses CONNECT.
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpStream},
    path::PathBuf,
    time::Duration,
};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Http,
    Https,
    Socks5,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub kind: Kind,
    pub address: String,
    #[serde(default)]
    pub credential_file: Option<PathBuf>,
}
impl Settings {
    pub fn validate(&self) -> io::Result<()> {
        crate::settings::endpoint(&self.address)?;
        if self.address.contains(['@', '/', '\\', '\r', '\n'])
            || self.address.chars().any(char::is_whitespace)
        {
            return Err(io::Error::other(
                "Proxy address must be HOST:PORT without credentials or URL path",
            ));
        }
        if self
            .credential_file
            .as_ref()
            .is_some_and(|p| !p.is_absolute())
        {
            return Err(io::Error::other(
                "Proxy credential file must be an absolute path",
            ));
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize, Default)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.password.clear();
    }
}
pub enum Stream {
    Plain(TcpStream),
    #[cfg(windows)]
    Tls(Box<native_tls::TlsStream<TcpStream>>),
}
impl Stream {
    pub fn socket(&self) -> &TcpStream {
        match self {
            Self::Plain(s) => s,
            #[cfg(windows)]
            Self::Tls(s) => s.get_ref(),
        }
    }
    pub fn shutdown(&self) {
        let _ = self.socket().shutdown(Shutdown::Both);
    }
}
impl Read for Stream {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(data),
            #[cfg(windows)]
            Self::Tls(s) => s.read(data),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(data),
            #[cfg(windows)]
            Self::Tls(s) => s.write(data),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            #[cfg(windows)]
            Self::Tls(s) => s.flush(),
        }
    }
}
pub fn connect(target: &str, proxy: Option<&Settings>, timeout: Duration) -> io::Result<Stream> {
    let Some(proxy) = proxy else {
        return crate::client::connect_timeout(target, timeout).map(Stream::Plain);
    };
    proxy.validate()?;
    crate::settings::endpoint(target)?;
    let credentials = if let Some(path) = &proxy.credential_file {
        crate::credentials::load(path)?
    } else {
        Credentials::default()
    };
    let socket = crate::client::connect_timeout(&proxy.address, timeout)?;
    socket.set_read_timeout(Some(timeout))?;
    socket.set_write_timeout(Some(timeout))?;
    let mut stream = match proxy.kind {
        Kind::Https => {
            #[cfg(windows)]
            {
                let (host, _) = crate::settings::endpoint(&proxy.address)?;
                let connector = native_tls::TlsConnector::builder()
                    .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
                    .build()
                    .map_err(io::Error::other)?;
                Stream::Tls(Box::new(connector.connect(&host, socket).map_err(
                    |_| io::Error::other("HTTPS proxy TLS handshake/certificate validation failed"),
                )?))
            }
            #[cfg(not(windows))]
            {
                return Err(io::Error::other(
                    "HTTPS proxy requires the Windows TLS adapter",
                ));
            }
        }
        _ => Stream::Plain(socket),
    };
    negotiate(&mut stream, target, &proxy.kind, &credentials)?;
    crate::diagnostics::event(
        "proxy.connected",
        serde_json::json!({"kind":proxy.kind,"proxy":proxy.address,"target":target}),
    );
    Ok(stream)
}
pub fn negotiate(
    stream: &mut (impl Read + Write),
    target: &str,
    kind: &Kind,
    auth: &Credentials,
) -> io::Result<()> {
    let (host, port) = crate::settings::endpoint(target)?;
    match kind {
        Kind::Http | Kind::Https => {
            use base64::Engine;
            if auth.username.contains(':') {
                return Err(io::Error::other("HTTP proxy username cannot contain ':'"));
            }
            let header = if auth.username.is_empty() && auth.password.is_empty() {
                String::new()
            } else {
                format!(
                    "Proxy-Authorization: Basic {}\r\n",
                    base64::engine::general_purpose::STANDARD
                        .encode(format!("{}:{}", auth.username, auth.password))
                )
            };
            write!(
                stream,
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{header}\r\n"
            )?;
            stream.flush()?;
            let mut reply = Vec::new();
            let mut byte = [0];
            while !reply.ends_with(b"\r\n\r\n") {
                if reply.len() >= 32768 {
                    return Err(io::Error::other(
                        "HTTP proxy response headers exceed 32 KiB",
                    ));
                }
                stream.read_exact(&mut byte)?;
                reply.push(byte[0]);
            }
            let line = std::str::from_utf8(reply.split(|b| *b == b'\n').next().unwrap_or_default())
                .map_err(|_| io::Error::other("Invalid HTTP proxy status line"))?;
            let mut parts = line.split_whitespace();
            let version = parts.next().unwrap_or_default();
            let status = parts
                .next()
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(0);
            if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || !(200..300).contains(&status) {
                return Err(io::Error::other(format!(
                    "HTTP CONNECT proxy rejected request (status {status}); check credentials and allowed destination ports"
                )));
            }
        }
        Kind::Socks5 => {
            let authenticated = !auth.username.is_empty() || !auth.password.is_empty();
            let method = if authenticated { 2 } else { 0 };
            stream.write_all(&[5, 1, method])?;
            let mut answer = [0; 2];
            stream.read_exact(&mut answer)?;
            if answer != [5, method] {
                return Err(io::Error::other(
                    "SOCKS5 proxy rejected the authentication method",
                ));
            }
            if authenticated {
                let user = auth.username.as_bytes();
                let pass = auth.password.as_bytes();
                if user.is_empty() || user.len() > 255 || pass.is_empty() || pass.len() > 255 {
                    return Err(io::Error::other(
                        "SOCKS5 username and password must contain 1..255 UTF-8 bytes",
                    ));
                }
                let mut packet = vec![1, user.len() as u8];
                packet.extend_from_slice(user);
                packet.push(pass.len() as u8);
                packet.extend_from_slice(pass);
                stream.write_all(&packet)?;
                stream.read_exact(&mut answer)?;
                if answer != [1, 0] {
                    return Err(io::Error::other(
                        "SOCKS5 username/password authentication failed",
                    ));
                }
            }
            let mut packet = vec![5, 1, 0];
            match host.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(ip)) => {
                    packet.push(1);
                    packet.extend_from_slice(&ip.octets());
                }
                Ok(std::net::IpAddr::V6(ip)) => {
                    packet.push(4);
                    packet.extend_from_slice(&ip.octets());
                }
                Err(_) => {
                    if host.len() > 255 {
                        return Err(io::Error::other("SOCKS5 destination name is too long"));
                    }
                    packet.extend_from_slice(&[3, host.len() as u8]);
                    packet.extend_from_slice(host.as_bytes());
                }
            }
            packet.extend_from_slice(&port.to_be_bytes());
            stream.write_all(&packet)?;
            let mut header = [0; 4];
            stream.read_exact(&mut header)?;
            if header[0] != 5 || header[1] != 0 || header[2] != 0 {
                return Err(io::Error::other(format!(
                    "SOCKS5 CONNECT failed (status {})",
                    header[1]
                )));
            }
            let length = match header[3] {
                1 => 4,
                4 => 16,
                3 => {
                    stream.read_exact(&mut answer[..1])?;
                    answer[0] as usize
                }
                _ => return Err(io::Error::other("Invalid SOCKS5 bound address")),
            };
            let mut bound = vec![0; length + 2];
            stream.read_exact(&mut bound)?;
        }
    }
    Ok(())
}
