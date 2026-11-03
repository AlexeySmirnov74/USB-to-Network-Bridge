#![cfg(windows)]
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    thread,
    time::Duration,
};
use usbbridge::{
    proxy::{self, Credentials, Kind, Settings},
    proxy_relay,
};
struct Temporary(PathBuf);
#[test]
fn dropping_a_relay_closes_all_active_local_and_proxy_sockets() {
    const CONNECTIONS: usize = 16;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_address = listener.local_addr().unwrap().to_string();
    let fixture = thread::spawn(move || {
        let mut workers = Vec::new();
        for _ in 0..CONNECTIONS {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            workers.push(thread::spawn(move || {
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 4096);
                }
                socket
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .unwrap();
                socket.read_exact(&mut byte).unwrap();
                socket.write_all(&byte).unwrap();
                match socket.read(&mut byte) {
                    Ok(0) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                        ) => {}
                    result => panic!("proxy socket survived relay shutdown: {result:?}"),
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let relay = proxy_relay::Relay::start(
        "device.invalid:4321".into(),
        Settings {
            kind: Kind::Http,
            address: proxy_address,
            credential_file: None,
        },
    )
    .unwrap();
    let mut clients = Vec::new();
    for _ in 0..CONNECTIONS {
        let mut socket = TcpStream::connect(&relay.address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket.write_all(&[0x5a]).unwrap();
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        assert_eq!(byte, [0x5a]);
        clients.push(socket);
    }
    drop(relay);
    for mut socket in clients {
        let mut byte = [0];
        match socket.read(&mut byte) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                ) => {}
            result => panic!("local socket survived relay shutdown: {result:?}"),
        }
    }
    fixture.join().unwrap();
}
impl Temporary {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("usbbridge-proxy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn auth() -> Credentials {
    Credentials {
        username: "fixture-user".into(),
        password: "fixture-pass".into(),
    }
}
fn http(stream: &mut (impl Read + Write)) {
    use base64::Engine;
    let mut request = Vec::new();
    let mut byte = [0];
    while !request.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
        assert!(request.len() < 4096);
    }
    let text = String::from_utf8(request).unwrap();
    assert!(text.starts_with("CONNECT device.invalid:4321 HTTP/1.1\r\n"));
    assert!(text.contains(&format!(
        "Proxy-Authorization: Basic {}",
        base64::engine::general_purpose::STANDARD.encode("fixture-user:fixture-pass")
    )));
    for part in [
        b"HTTP/1.1 200 Connection".as_slice(),
        b" established\r\nProxy-Agent: fixture\r\n\r\n".as_slice(),
    ] {
        stream.write_all(part).unwrap();
    }
}
fn socks(stream: &mut (impl Read + Write)) {
    let mut greeting = [0; 3];
    stream.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 1, 2]);
    stream.write_all(&[5, 2]).unwrap();
    let mut header = [0; 2];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(header[0], 1);
    let mut user = vec![0; header[1] as usize];
    stream.read_exact(&mut user).unwrap();
    let mut length = [0];
    stream.read_exact(&mut length).unwrap();
    let mut password = vec![0; length[0] as usize];
    stream.read_exact(&mut password).unwrap();
    assert_eq!(user, b"fixture-user");
    assert_eq!(password, b"fixture-pass");
    stream.write_all(&[1, 0]).unwrap();
    let mut connect = [0; 5];
    stream.read_exact(&mut connect).unwrap();
    assert_eq!(&connect[..4], &[5, 1, 0, 3]);
    let mut host = vec![0; connect[4] as usize];
    stream.read_exact(&mut host).unwrap();
    assert_eq!(host, b"device.invalid");
    let mut port = [0; 2];
    stream.read_exact(&mut port).unwrap();
    assert_eq!(u16::from_be_bytes(port), 4321);
    let mut reply = vec![5, 0, 0, 4];
    reply.extend_from_slice(&[0; 18]);
    stream.write_all(&reply).unwrap();
}
fn echo(stream: &mut (impl Read + Write)) {
    let mut buffer = vec![0; 16384];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if stream.write_all(&buffer[..n]).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
fn exchange(mut socket: TcpStream) {
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let data: Vec<_> = (0..2 * 1024 * 1024)
        .map(|n| ((n * 17 + n / 251) % 256) as u8)
        .collect();
    let copy = data.clone();
    let mut writer = socket.try_clone().unwrap();
    let send = thread::spawn(move || writer.write_all(&copy).unwrap());
    let mut actual = vec![0; data.len()];
    socket.read_exact(&mut actual).unwrap();
    assert_eq!(actual, data);
    send.join().unwrap();
    socket.shutdown(Shutdown::Both).unwrap();
}
fn relay_roundtrip(kind: Kind) {
    let temporary = Temporary::new();
    let credential_file = usbbridge::credentials::save_at(&temporary.0, &auth()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let fixture_kind = kind.clone();
    let fixture = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        match fixture_kind {
            Kind::Socks5 => socks(&mut socket),
            _ => http(&mut socket),
        }
        echo(&mut socket);
    });
    let relay = proxy_relay::Relay::start(
        "device.invalid:4321".into(),
        Settings {
            kind,
            address,
            credential_file: Some(credential_file),
        },
    )
    .unwrap();
    exchange(TcpStream::connect(&relay.address).unwrap());
    drop(relay);
    fixture.join().unwrap();
}
#[test]
fn authenticated_http_relay_preserves_binary_stream_and_backpressure() {
    relay_roundtrip(Kind::Http)
}
#[test]
fn authenticated_socks5_relay_uses_remote_dns_and_preserves_binary_stream() {
    relay_roundtrip(Kind::Socks5)
}
#[test]
fn rejected_http_credentials_never_fall_back_to_direct_connection() {
    let temporary = Temporary::new();
    let credential_file = usbbridge::credentials::save_at(&temporary.0, &auth()).unwrap();
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let target = destination.local_addr().unwrap().to_string();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let fixture = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut b = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut b).unwrap();
            request.push(b[0]);
        }
        socket
            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
            .unwrap();
    });
    let error = proxy::connect(
        &target,
        Some(&Settings {
            kind: Kind::Http,
            address,
            credential_file: Some(credential_file),
        }),
        Duration::from_secs(2),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("407"));
    assert_eq!(
        destination.accept().err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    fixture.join().unwrap();
}
#[test]
fn socks5_refuses_authentication_downgrade() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 3];
        socket.read_exact(&mut request).unwrap();
        assert_eq!(request, [5, 1, 2]);
        socket.write_all(&[5, 0]).unwrap();
    });
    let mut socket = TcpStream::connect(address).unwrap();
    let error =
        proxy::negotiate(&mut socket, "device.invalid:4321", &Kind::Socks5, &auth()).unwrap_err();
    assert!(error.to_string().contains("authentication method"));
    fixture.join().unwrap();
}
#[test]
fn https_connect_and_tls_relay_work_without_changing_windows_trust() {
    let temporary = Temporary::new();
    let script = r#"$ErrorActionPreference='Stop';$dir=$env:USB_BRIDGE_TLS_FIXTURE;$rsa=[Security.Cryptography.RSA]::Create(2048);$request=[Security.Cryptography.X509Certificates.CertificateRequest]::new('CN=localhost',$rsa,[Security.Cryptography.HashAlgorithmName]::SHA256,[Security.Cryptography.RSASignaturePadding]::Pkcs1);$san=[Security.Cryptography.X509Certificates.SubjectAlternativeNameBuilder]::new();$san.AddDnsName('localhost');$request.CertificateExtensions.Add($san.Build());$request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($true,$false,0,$true));$cert=$request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddDays(-1),[DateTimeOffset]::UtcNow.AddDays(1));[IO.File]::WriteAllBytes((Join-Path $dir 'fixture.pfx'),$cert.Export([Security.Cryptography.X509Certificates.X509ContentType]::Pkcs12,'fixture'));[IO.File]::WriteAllBytes((Join-Path $dir 'fixture.der'),$cert.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert));$cert.Dispose();$rsa.Dispose()"#;
    let result = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", script])
        .env("USB_BRIDGE_TLS_FIXTURE", &temporary.0)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "Certificate fixture generation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let identity = native_tls::Identity::from_pkcs12(
        &std::fs::read(temporary.0.join("fixture.pfx")).unwrap(),
        "fixture",
    )
    .unwrap();
    let certificate =
        native_tls::Certificate::from_der(&std::fs::read(temporary.0.join("fixture.der")).unwrap())
            .unwrap();
    let acceptor = native_tls::TlsAcceptor::new(identity).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut tls = acceptor.accept(socket).unwrap();
        http(&mut tls);
        echo(&mut tls);
        let (socket, _) = listener.accept().unwrap();
        if let Ok(mut rejected) = acceptor.accept(socket) {
            let mut byte = [0];
            assert!(!matches!(rejected.read(&mut byte),Ok(n) if n>0));
        }
    });
    let connector = native_tls::TlsConnector::builder()
        .add_root_certificate(certificate)
        .build()
        .unwrap();
    let mut tls = proxy::Stream::Tls(Box::new(
        connector
            .connect("localhost", TcpStream::connect(address).unwrap())
            .unwrap(),
    ));
    proxy::negotiate(&mut tls, "device.invalid:4321", &Kind::Https, &auth()).unwrap();
    let local = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = local.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let pump = thread::spawn(move || {
        let (mut socket, _) = local.accept().unwrap();
        proxy_relay::pump(&mut socket, &mut tls, &stop).unwrap()
    });
    exchange(TcpStream::connect(endpoint).unwrap());
    let (sent, received) = pump.join().unwrap();
    assert_eq!(sent, 2 * 1024 * 1024);
    assert_eq!(received, sent);
    let error = proxy::connect(
        "device.invalid:4321",
        Some(&Settings {
            kind: Kind::Https,
            address: format!("localhost:{}", address.port()),
            credential_file: None,
        }),
        Duration::from_secs(3),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("certificate validation failed"));
    fixture.join().unwrap();
}
