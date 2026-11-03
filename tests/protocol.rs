use std::{
    io::{self, Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
};
use usbbridge::{
    backend::{MockProvider, mock_device},
    client,
    server::Server,
    wire::{self, Completion, Request, Submit},
};

fn request(seq: u32, direction: u32, length: usize) -> Submit {
    Submit {
        seq,
        devid: 0x10001,
        direction,
        endpoint: 1,
        flags: 0,
        length,
        start_frame: 0,
        packet_count: u32::MAX,
        interval: 0,
        setup: [0; 8],
        data: if direction == 0 {
            vec![0x5a; length]
        } else {
            Vec::new()
        },
        iso: Vec::new(),
    }
}
fn fixture(n: usize, test: impl FnOnce(&str)) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = Arc::new(Server::new(
        Arc::new(MockProvider),
        vec!["all".into()],
        Vec::new(),
    ));
    let accept = thread::spawn(move || {
        let mut connections = Vec::new();
        for _ in 0..n {
            let (stream, _) = listener.accept().unwrap();
            let server = server.clone();
            connections.push(thread::spawn(move || {
                let _ = server.connection(stream);
            }));
        }
        for c in connections {
            c.join().unwrap();
        }
    });
    test(&address);
    accept.join().unwrap();
}
#[test]
fn benchmark_receive_failure_releases_a_sender_waiting_for_credits() {
    use std::{sync::mpsc, time::Duration};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let peer = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut import = [0; 40];
        socket.read_exact(&mut import).unwrap();
        socket.write_all(&wire::operation(3, 0)).unwrap();
        socket.write_all(&mock_device().encode().unwrap()).unwrap();
        assert!(matches!(
            wire::read_request(&mut socket),
            Ok(Request::Submit(_))
        ));
        // Reject the first completion while the benchmark has exhausted its one credit.
        socket.write_all(&[0; 48]).unwrap();
        let mut tail = Vec::new();
        let _ = socket.read_to_end(&mut tail);
    });
    let (sent, result) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        sent.send(client::bench(&address, 1, 65536, 1, true).is_err())
            .unwrap();
    });
    assert!(
        result
            .recv_timeout(Duration::from_secs(3))
            .expect("benchmark retained a dead receiver's credit sender")
    );
    worker.join().unwrap();
    peer.join().unwrap();
}
#[test]
fn live_publication_changes_preserve_unrelated_stream_and_withdraw_only_selected_identity() {
    use std::{path::PathBuf, time::Duration};
    use usbbridge::{
        backend::{Provider, Session},
        publication::{Snapshot, Store},
        settings,
    };
    struct Published(Arc<Store>);
    impl Provider for Published {
        fn devices(&self) -> io::Result<Vec<wire::Device>> {
            Ok(self.0.devices())
        }
        fn open(
            &self,
            id: &str,
            tx: std::sync::mpsc::SyncSender<Completion>,
        ) -> io::Result<Arc<dyn Session>> {
            MockProvider.open(id, tx)
        }
    }
    let path: PathBuf =
        std::env::temp_dir().join(format!("publication-stream-{}.json", uuid::Uuid::new_v4()));
    let mut camera = mock_device();
    camera.path = "USB\\camera".into();
    let mut other = camera.clone();
    other.path = "USB\\other".into();
    other.busid = "other-2".into();
    let mut snapshot = Snapshot {
        version: 1,
        revision: uuid::Uuid::new_v4().to_string(),
        devices: vec![camera.clone()],
        local_when_idle: Vec::new(),
    };
    settings::save(&path, &snapshot).unwrap();
    let store = Arc::new(Store::load(&path).unwrap());
    let server = Arc::new(
        Server::new(
            Arc::new(Published(store.clone())),
            vec!["all".into()],
            Vec::new(),
        )
        .with_presence_interval(Duration::from_millis(10)),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let _ = server.connection(stream);
    });
    let (mut stream, _) = client::import(&address, &camera.busid).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    // All transfers use the original TCP import; a reconnect would fail this test.
    for seq in 1..=6 {
        snapshot.devices = if seq % 2 == 0 {
            vec![camera.clone()]
        } else {
            vec![camera.clone(), other.clone()]
        };
        snapshot.revision = uuid::Uuid::new_v4().to_string();
        settings::save(&path, &snapshot).unwrap();
        assert_eq!(store.devices().len(), snapshot.devices.len());
        thread::sleep(Duration::from_millis(35));
        stream.write_all(&request(seq, 1, 16).encode()).unwrap();
        let mut reply = [0; 48];
        stream.read_exact(&mut reply).unwrap();
        let mut data = [0; 16];
        stream.read_exact(&mut data).unwrap();
        assert_eq!(wire::u32be(&reply[4..]), seq);
    }
    // Removing the imported identity, however, must close its own session.
    snapshot.devices = vec![other];
    snapshot.revision = uuid::Uuid::new_v4().to_string();
    settings::save(&path, &snapshot).unwrap();
    let mut reply = [0; 48];
    assert!(stream.read_exact(&mut reply).is_err());
    drop(stream);
    worker.join().unwrap();
    std::fs::remove_file(path).unwrap();
}
#[test]
fn device_list_over_tcp() {
    fixture(1, |address| {
        let devices = client::list(address).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].busid, "test-1");
        assert_eq!(devices[0].interfaces, vec![[0xff, 0, 0, 0]]);
    });
}
#[test]
fn dashboard_session_tracks_real_peer_and_releases_after_disconnect() {
    use std::time::{Duration, Instant};
    use usbbridge::{host_status::Sessions, settings};
    let path = std::env::temp_dir().join(format!("usbbridge-{}.status.json", uuid::Uuid::new_v4()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = Arc::new(
        Server::new(Arc::new(MockProvider), vec!["all".into()], Vec::new())
            .with_status_file(path.clone()),
    );
    let worker = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let _ = server.connection(socket);
    });
    let (stream, _) = client::import(&address.to_string(), "test-1").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let snapshot = loop {
        let snapshot: Sessions = settings::read(&path).unwrap();
        if !snapshot.connections.is_empty() {
            break snapshot;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(snapshot.process_id, std::process::id());
    assert_eq!(snapshot.connections[0].busid, "test-1");
    assert_eq!(snapshot.connections[0].client, stream.local_addr().unwrap());
    assert_eq!(snapshot.connections[0].server, address);
    drop(stream);
    worker.join().unwrap();
    assert!(
        settings::read::<Sessions>(&path)
            .unwrap()
            .connections
            .is_empty()
    );
    std::fs::remove_file(path).unwrap();
}
#[test]
fn idle_and_partial_transfers_survive_cancellation_poll_timeouts() {
    fixture(1, |address| {
        use std::time::Duration;
        let (mut stream, _) = client::import(address, "test-1").unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let out = request(70, 0, 16);
        let header = out.encode();
        stream.write_all(&header[..17]).unwrap();
        thread::sleep(Duration::from_millis(1150));
        stream.write_all(&header[17..]).unwrap();
        stream.write_all(&out.data).unwrap();
        let mut reply = [0; 48];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(wire::u32be(&reply[4..]), 70);
        thread::sleep(Duration::from_millis(1150));
        stream.write_all(&request(71, 1, 16).encode()).unwrap();
        stream.read_exact(&mut reply).unwrap();
        let mut data = [0; 16];
        stream.read_exact(&mut data).unwrap();
        assert_eq!(&data[..], &out.data);
    });
}
#[test]
fn physical_removal_closes_session_releases_lease_and_allows_replug() {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    use usbbridge::backend::{Provider, Session};
    struct Toggle(Arc<AtomicBool>);
    impl Provider for Toggle {
        fn devices(&self) -> io::Result<Vec<wire::Device>> {
            Ok(if self.0.load(Ordering::Acquire) {
                vec![mock_device()]
            } else {
                Vec::new()
            })
        }
        fn open(
            &self,
            id: &str,
            tx: std::sync::mpsc::SyncSender<Completion>,
        ) -> io::Result<Arc<dyn Session>> {
            MockProvider.open(id, tx)
        }
    }
    let present = Arc::new(AtomicBool::new(true));
    let server = Server::new(
        Arc::new(Toggle(present.clone())),
        vec!["all".into()],
        Vec::new(),
    )
    .with_presence_interval(Duration::from_millis(20));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (closed, wait_closed) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let _ = server.connection(stream);
        closed.send(()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let _ = server.connection(stream);
    });
    let (mut stream, _) = client::import(&address, "test-1").unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    present.store(false, Ordering::Release);
    let mut header = [0; 48];
    assert!(stream.read_exact(&mut header).is_err());
    wait_closed.recv_timeout(Duration::from_secs(2)).unwrap();
    present.store(true, Ordering::Release);
    let (mut replugged, _) = client::import(&address, "test-1").unwrap();
    replugged
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    replugged.write_all(&request(42, 1, 4096).encode()).unwrap();
    replugged.read_exact(&mut header).unwrap();
    assert_eq!(wire::u32be(&header[4..]), 42);
    assert_eq!(wire::u32be(&header[24..]), 4096);
    replugged.read_exact(&mut vec![0; 4096]).unwrap();
    drop(replugged);
    worker.join().unwrap();
}
#[test]
fn native_exports_follow_identity_when_device_moves_and_reject_old_port_replacement() {
    use usbbridge::backend::{Provider, Session};
    struct Moving(Arc<std::sync::Mutex<Vec<wire::Device>>>);
    impl Provider for Moving {
        fn devices(&self) -> io::Result<Vec<wire::Device>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn open(
            &self,
            _: &str,
            _: std::sync::mpsc::SyncSender<Completion>,
        ) -> io::Result<Arc<dyn Session>> {
            Err(io::Error::other("This fixture only lists devices"))
        }
    }
    let original = mock_device();
    let devices = Arc::new(std::sync::Mutex::new(vec![original.clone()]));
    let server = Server::new(
        Arc::new(Moving(devices.clone())),
        vec![original.busid.clone()],
        Vec::new(),
    )
    .with_export_identities([original.path.clone()].into_iter().collect());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let worker = thread::spawn(move || {
        for _ in 0..3 {
            let (stream, _) = listener.accept().unwrap();
            server.connection(stream).unwrap();
        }
    });
    assert_eq!(client::list(&address).unwrap(), vec![original.clone()]);
    devices.lock().unwrap().clear();
    assert!(client::list(&address).unwrap().is_empty());
    let mut moved = original.clone();
    moved.busid = "3-9".into();
    moved.bus = 3;
    moved.address = 9;
    let mut replacement = original;
    replacement.path = "another-physical-device".into();
    *devices.lock().unwrap() = vec![replacement, moved.clone()];
    assert_eq!(client::list(&address).unwrap(), vec![moved]);
    worker.join().unwrap();
}
#[test]
fn bidirectional_bulk_and_unlink() {
    fixture(1, |address| {
        let (mut stream, _) = client::import(address, "test-1").unwrap();
        let out = request(10, 0, 16384);
        stream.write_all(&out.encode()).unwrap();
        stream.write_all(&out.data).unwrap();
        let input = request(11, 1, 16384);
        stream.write_all(&input.encode()).unwrap();
        let mut header = [0; 48];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(wire::u32be(&header[4..]), 10);
        assert_eq!(wire::u32be(&header[24..]), 16384);
        stream.read_exact(&mut header).unwrap();
        assert_eq!(wire::u32be(&header[4..]), 11);
        let mut data = vec![0; 16384];
        stream.read_exact(&mut data).unwrap();
        assert_eq!(data, out.data);
        let mut unlink = [0; 48];
        wire::put32(&mut unlink, wire::UNLINK);
        wire::put32(&mut unlink[4..], 12);
        wire::put32(&mut unlink[20..], 11);
        stream.write_all(&unlink).unwrap();
        stream.read_exact(&mut header).unwrap();
        assert_eq!(wire::u32be(&header), wire::RET_UNLINK);
        assert_eq!(wire::u32be(&header[4..]), 12);
        assert_eq!(wire::u32be(&header[20..]), 0);
    });
}
#[test]
fn exclusive_device_lease() {
    fixture(2, |address| {
        let (first, _) = client::import(address, "test-1").unwrap();
        assert!(client::import(address, "test-1").is_err());
        drop(first);
    });
}
#[test]
fn pipelined_out_benchmark() {
    fixture(1, |address| {
        let result = client::bench(address, 4, 65536, 16, false).unwrap();
        assert_eq!(result.bytes, 4 * 1024 * 1024);
    });
}
#[test]
fn pipelined_in_benchmark() {
    fixture(1, |address| {
        let result = client::bench(address, 4, 65536, 16, true).unwrap();
        assert_eq!(result.bytes, 4 * 1024 * 1024);
    });
}
#[test]
fn reject_oversize_before_allocation() {
    let mut header = request(1, 0, wire::MAX_TRANSFER + 1).encode();
    assert!(wire::read_request(&mut &header[..]).is_err());
    wire::put32(&mut header[24..], 0);
    wire::put32(&mut header[32..], 65535);
    assert!(wire::read_request(&mut &header[..]).is_err());
}
#[test]
fn preserve_non_iso_packet_sentinel() {
    let header = request(7, 1, 4096).encode();
    let Request::Submit(req) = wire::read_request(&mut &header[..]).unwrap() else {
        panic!()
    };
    assert_eq!(req.packet_count, u32::MAX);
    assert!(req.iso.is_empty());
}
#[test]
fn reject_iso_bounds() {
    let mut req = request(9, 1, 1024);
    req.packet_count = 1;
    let mut bytes = req.encode().to_vec();
    for value in [1000u32, 100, 0, 0] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    assert!(wire::read_request(&mut &bytes[..]).is_err());
}
#[test]
fn device_roundtrip_and_string_validation() {
    let device = mock_device();
    let bytes = device.encode().unwrap();
    let decoded = wire::Device::decode(&bytes).unwrap();
    assert_eq!(decoded.vendor, device.vendor);
    assert_eq!(decoded.busid, device.busid);
    assert!(wire::fixed_string(&[b'x'; 32]).is_err());
}
#[test]
fn partial_vectored_writes_and_wire_endianness() {
    struct ShortWriter(Vec<u8>);
    impl Write for ShortWriter {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            let n = b.len().min(3);
            self.0.extend_from_slice(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let req = request(0x12345678, 1, 1024);
    let completion = Completion::submit(&req, 0, 4, vec![9, 1, 2, 3, 4, 9], 1, Vec::new());
    let mut writer = ShortWriter(Vec::new());
    completion.write(&mut writer).unwrap();
    assert_eq!(writer.0.len(), 52);
    assert_eq!(&writer.0[..8], &[0, 0, 0, 3, 0x12, 0x34, 0x56, 0x78]);
    assert_eq!(&writer.0[48..], &[1, 2, 3, 4]);
}
#[test]
fn memory_budget_retained_until_completion_is_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use usbbridge::budget::MemoryLease;
    let used = Arc::new(AtomicUsize::new(0));
    let req = request(1, 1, 8);
    let mut completion = Completion::submit(&req, 0, 8, vec![0; 8], 0, Vec::new());
    completion.memory = MemoryLease::reserve(&used, 8, 10);
    assert!(MemoryLease::reserve(&used, 3, 10).is_none());
    let extra = MemoryLease::reserve(&used, 2, 10).unwrap();
    assert_eq!(used.load(Ordering::Relaxed), 10);
    drop(completion);
    assert_eq!(used.load(Ordering::Relaxed), 2);
    drop(extra);
    assert_eq!(used.load(Ordering::Relaxed), 0);
}
