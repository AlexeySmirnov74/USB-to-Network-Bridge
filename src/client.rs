use crate::{
    tune_socket,
    wire::{self, Device, Submit},
};
use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    thread,
    time::{Duration, Instant},
};

pub fn connect(address: &str) -> io::Result<TcpStream> {
    connect_timeout(address, Duration::from_secs(10))
}
pub fn connect_timeout(address: &str, timeout: Duration) -> io::Result<TcpStream> {
    let addresses = address.to_socket_addrs()?;
    let mut last = io::Error::from(io::ErrorKind::AddrNotAvailable);
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => {
                tune_socket(&stream)?;
                return Ok(stream);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}
pub fn list(address: &str) -> io::Result<Vec<Device>> {
    list_timeout(address, Duration::from_secs(10))
}
pub fn list_timeout(address: &str, timeout: Duration) -> io::Result<Vec<Device>> {
    let mut stream = connect_timeout(address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    list_stream(&mut stream)
}
pub fn list_with_proxy(
    address: &str,
    proxy: Option<&crate::proxy::Settings>,
    timeout: Duration,
) -> io::Result<Vec<Device>> {
    if proxy.is_none() {
        return list_timeout(address, timeout);
    }
    let mut stream = crate::proxy::connect(address, proxy, timeout)?;
    list_stream(&mut stream)
}
fn list_stream(stream: &mut impl ReadWrite) -> io::Result<Vec<Device>> {
    stream.write_all(&wire::operation(wire::DEVLIST, 0))?;
    if wire::read_operation(stream)? != (5, 0) {
        return Err(wire::invalid("device list request rejected"));
    }
    let mut n = [0; 4];
    stream.read_exact(&mut n)?;
    let count = wire::u32be(&n);
    if count > 65535 {
        return Err(wire::invalid("invalid device list size"));
    }
    let mut devices = Vec::new();
    for _ in 0..count {
        let mut b = [0; 312];
        stream.read_exact(&mut b)?;
        let mut d = Device::decode(&b)?;
        for interface in &mut d.interfaces {
            stream.read_exact(interface)?;
        }
        devices.push(d);
    }
    Ok(devices)
}
trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}
pub fn import(address: &str, busid: &str) -> io::Result<(TcpStream, Device)> {
    let mut stream = connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut request = [0; 40];
    request[..8].copy_from_slice(&wire::operation(wire::IMPORT, 0));
    wire::put_string(&mut request[8..], busid)?;
    stream.write_all(&request)?;
    let (op, status) = wire::read_operation(&mut stream)?;
    if op != 3 || status != 0 {
        return Err(io::Error::other(
            "device is unavailable, busy, or not exported",
        ));
    }
    let mut b = [0; 312];
    stream.read_exact(&mut b)?;
    stream.set_read_timeout(None)?;
    Ok((stream, Device::decode(&b)?))
}
pub struct BenchResult {
    pub bytes: u64,
    pub seconds: f64,
    pub mib_per_second: f64,
    pub direction: &'static str,
}
pub fn bench(
    address: &str,
    megabytes: u64,
    block: usize,
    depth: usize,
    direction_in: bool,
) -> io::Result<BenchResult> {
    if block == 0 || block > wire::MAX_TRANSFER || !(1..=1024).contains(&depth) || megabytes == 0 {
        return Err(wire::invalid("invalid benchmark dimensions"));
    }
    let total_bytes = megabytes
        .checked_mul(1024 * 1024)
        .ok_or_else(|| wire::invalid("benchmark size overflow"))?;
    let count = total_bytes.div_ceil(block as u64);
    if count > u32::MAX as u64 {
        return Err(wire::invalid("benchmark sequence count overflow"));
    }
    let (stream, device) = import(address, "test-1")?;
    let mut output = stream.try_clone()?;
    let mut input = stream;
    input.set_read_timeout(Some(Duration::from_secs(30)))?;
    let (returned, permits) = std::sync::mpsc::sync_channel(depth);
    for _ in 0..depth {
        returned.send(()).unwrap();
    }
    let returned_writer = returned.clone();
    // The receiver must own the last sender. A failed read must wake a writer
    // waiting for credits instead of retaining both the session and its buffers.
    drop(returned);
    let begin = Instant::now();
    let receive = thread::spawn(move || -> io::Result<u64> {
        let mut bytes = 0;
        let mut payload = vec![0; block];
        for expected in 0..count {
            let mut b = [0; 48];
            input.read_exact(&mut b)?;
            if wire::u32be(&b) != wire::RET_SUBMIT
                || wire::u32be(&b[4..]) != expected as u32
                || wire::u32be(&b[20..]) != 0
            {
                return Err(wire::invalid("benchmark completion mismatch"));
            }
            let actual = wire::u32be(&b[24..]) as usize;
            if actual > block {
                return Err(wire::invalid("benchmark payload overflow"));
            }
            if direction_in {
                input.read_exact(&mut payload[..actual])?;
            }
            bytes += actual as u64;
            let _ = returned_writer.send(());
        }
        Ok(bytes)
    });
    let payload = vec![0x5a; block];
    let mut sending = Ok(());
    for seq in 0..count {
        if permits.recv().is_err() {
            sending = Err(io::ErrorKind::BrokenPipe.into());
            break;
        }
        let req = Submit {
            seq: seq as u32,
            devid: device.devid(),
            direction: direction_in as u32,
            endpoint: 1,
            flags: 0,
            length: block,
            start_frame: 0,
            packet_count: 0,
            interval: 0,
            setup: [0; 8],
            data: Vec::new(),
            iso: Vec::new(),
        };
        let header = req.encode();
        let mut slices = [
            std::io::IoSlice::new(&header),
            std::io::IoSlice::new(if direction_in { &[] } else { &payload }),
        ];
        if let Err(e) = wire::write_vectored_all(&mut output, &mut slices) {
            sending = Err(e);
            break;
        }
    }
    // Stop accepting unused permits before joining the receiver, avoiding a full
    // credit channel on the final completion.
    drop(permits);
    if sending.is_err() {
        let _ = output.shutdown(std::net::Shutdown::Both);
    }
    let bytes = receive
        .join()
        .map_err(|_| io::Error::other("benchmark receiver panicked"))??;
    sending?;
    let seconds = begin.elapsed().as_secs_f64();
    Ok(BenchResult {
        bytes,
        seconds,
        mib_per_second: bytes as f64 / (1024. * 1024.) / seconds,
        direction: if direction_in { "IN" } else { "OUT" },
    })
}
