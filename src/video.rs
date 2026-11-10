//! Authenticated MJPEG ingress feeding a persistent local UVC device.
//! Network loss changes the image, not the Windows device topology.
use crate::{
    uvc::{self, Config, Frames, UvcProvider},
    wire,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    net::{IpAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
type Hmac256 = Hmac<Sha256>;
const MAGIC: &[u8; 8] = b"UBVC\0\0\0\x01";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverSettings {
    pub enabled: bool,
    pub camera: Config,
    pub usb_port: u16,
    pub video_port: u16,
    pub allowed_ips: Vec<IpAddr>,
    pub key_file: PathBuf,
}
impl ReceiverSettings {
    pub fn validate(&self) -> io::Result<()> {
        self.camera.validate()?;
        if self.usb_port == 0
            || self.video_port == 0
            || self.usb_port == self.video_port
            || self.allowed_ips.is_empty()
            || !self.key_file.is_absolute()
        {
            return Err(wire::invalid(
                "receiver needs two distinct ports, allowed client IPs and an absolute key file",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SenderSettings {
    pub enabled: bool,
    pub server: String,
    pub camera: String,
    pub quality: u8,
    pub key_file: PathBuf,
}
impl SenderSettings {
    pub fn validate(&self) -> io::Result<()> {
        crate::settings::endpoint(&self.server)?;
        if self.camera.is_empty()
            || !(1..=100).contains(&self.quality)
            || !self.key_file.is_absolute()
        {
            return Err(wire::invalid(
                "sender needs a source, quality 1..100 and an absolute key file",
            ));
        }
        Ok(())
    }
}
fn role_path(config: &Path, role: &str) -> PathBuf {
    let stem = config.file_stem().unwrap_or_default().to_string_lossy();
    let stem = stem.strip_suffix(".local").unwrap_or(&stem);
    config.with_file_name(format!("{stem}.video-{role}.local.json"))
}
pub fn receiver_path(config: &Path) -> PathBuf {
    role_path(config, "receive")
}
pub fn sender_path(config: &Path) -> PathBuf {
    role_path(config, "send")
}

fn expire_status(value: &mut serde_json::Value, now: u64) -> bool {
    let fresh = value["updated_unix_ms"]
        .as_u64()
        .is_some_and(|t| t <= now.saturating_add(5000) && now.saturating_sub(t) <= 5000);
    if !fresh {
        for name in [
            "running",
            "connected",
            "source_connected",
            "device_attached",
        ] {
            value[name] = serde_json::json!(false);
        }
    }
    fresh
}
pub fn status(config: &Path) -> serde_json::Value {
    let mut value =
        crate::settings::read::<serde_json::Value>(&config.with_extension("status.json"))
            .unwrap_or_else(|_| serde_json::json!({"running":false}));
    let fresh = expire_status(&mut value, crate::recovery::timestamp());
    serde_json::json!({"fresh":fresh,"status":value})
}
fn key(path: &Path) -> io::Result<[u8; 32]> {
    let text = if path.extension().is_some_and(|x| x == "credential") {
        crate::credentials::load(path)?.password.clone()
    } else {
        if fs::metadata(path)?.len() > 4096 {
            return Err(wire::invalid("invalid video connection key file"));
        }
        fs::read_to_string(path)?.trim().to_owned()
    };
    if !(16..=128).contains(&text.len()) || text.chars().any(char::is_control) {
        return Err(wire::invalid(
            "video connection key must contain 16..128 characters",
        ));
    }
    Ok(Sha256::digest(text.as_bytes()).into())
}
fn mac(secret: &[u8; 32], parts: &[&[u8]]) -> Hmac256 {
    let mut value = Hmac256::new_from_slice(secret).expect("SHA256 accepts a 32 byte key");
    for part in parts {
        value.update(part);
    }
    value
}
fn read_exact(stream: &mut TcpStream, bytes: &mut [u8]) -> io::Result<()> {
    let due = Instant::now() + Duration::from_secs(10);
    let mut offset = 0;
    while offset < bytes.len() {
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) =>
            {
                if Instant::now() >= due {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "video frame stalled",
                    ));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn hello(config: &Config) -> [u8; 48] {
    let mut h = [0; 48];
    h[..8].copy_from_slice(MAGIC);
    h[8..10].copy_from_slice(&config.width.to_be_bytes());
    h[10..12].copy_from_slice(&config.height.to_be_bytes());
    h[12..16].copy_from_slice(&config.fps.to_be_bytes());
    h[16..32].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    h[32..48].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    h
}
fn session_key(secret: &[u8; 32], hello: &[u8; 48]) -> [u8; 32] {
    mac(secret, &[b"USBBridge video session", hello])
        .finalize()
        .into_bytes()
        .into()
}
fn configure_socket(stream: &TcpStream) -> io::Result<()> {
    crate::tune_socket(stream)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))
}
struct Ownership(Arc<AtomicBool>, Arc<Frames>);
impl Drop for Ownership {
    fn drop(&mut self) {
        self.1.clear();
        self.0.store(false, Ordering::Release);
    }
}
fn ingest(
    mut stream: TcpStream,
    frames: Arc<Frames>,
    secret: &[u8; 32],
    active: Arc<AtomicBool>,
) -> io::Result<()> {
    configure_socket(&stream)?;
    let hello = hello(frames.config());
    stream.write_all(&hello)?;
    let mut signature = [0; 32];
    read_exact(&mut stream, &mut signature)?;
    mac(secret, &[b"USBBridge video authenticate", &hello])
        .verify_slice(&signature)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "video connection authentication failed",
            )
        })?;
    if active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(io::Error::other("another video sender is active"));
    }
    let _owner = Ownership(active, frames.clone());
    let session = session_key(secret, &hello);
    let mut sequence = 0u64;
    crate::diagnostics::event(
        "video.source.connected",
        serde_json::json!({"peer":stream.peer_addr().ok()}),
    );
    stream.write_all(b"OKAY")?;
    stream.write_all(
        &mac(secret, &[b"USBBridge video receiver", &hello, &signature])
            .finalize()
            .into_bytes(),
    )?;
    loop {
        let mut header = [0; 44];
        read_exact(&mut stream, &mut header)?;
        let seq = u64::from_be_bytes(header[..8].try_into().unwrap());
        let length = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
        if seq <= sequence || length < 4 || length > frames.config().frame_limit() {
            return Err(wire::invalid("video frame sequence/length is invalid"));
        }
        let mut bytes = vec![0; length];
        read_exact(&mut stream, &mut bytes)?;
        mac(&session, &[&header[..12], &bytes])
            .verify_slice(&header[12..])
            .map_err(|_| wire::invalid("video frame authentication failed"))?;
        frames.accept(bytes)?;
        sequence = seq;
    }
}
fn ingress(
    listener: TcpListener,
    frames: Arc<Frames>,
    secret: [u8; 32],
    allowed: Vec<IpAddr>,
) -> io::Result<()> {
    let active = Arc::new(AtomicBool::new(false));
    let workers = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let stream = stream?;
        let peer = stream.peer_addr()?;
        if !peer.ip().is_loopback() && !allowed.contains(&peer.ip()) {
            crate::diagnostics::event("video.peer.denied", serde_json::json!({"peer":peer}));
            continue;
        }
        if workers.fetch_add(1, Ordering::AcqRel) >= 4 {
            workers.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let count = workers.clone();
        let frames = frames.clone();
        let active = active.clone();
        let started = thread::Builder::new()
            .name("video-ingress".into())
            .spawn(move || {
                struct Slot(Arc<AtomicUsize>);
                impl Drop for Slot {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _slot = Slot(count);
                if let Err(error) = ingest(stream, frames, &secret, active) {
                    crate::diagnostics::event(
                        "video.source.disconnected",
                        serde_json::json!({"peer":peer,"error":error.to_string()}),
                    );
                }
            });
        if let Err(error) = started {
            workers.fetch_sub(1, Ordering::AcqRel);
            return Err(error);
        }
    }
    Ok(())
}
#[cfg(windows)]
pub fn receive(config: &ReceiverSettings, path: &Path) -> io::Result<()> {
    config.validate()?;
    if !config.enabled {
        return Ok(());
    }
    let _lease = crate::settings::acquire_file_lock(&path.with_extension("agent.lock"))?;
    crate::diagnostics::init(path, "video-receiver");
    let secret = key(&config.key_file)?;
    let frames = Frames::new(config.camera.clone())?;
    let usb = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.usb_port))?;
    let video = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, config.video_port))?;
    let state = frames.clone();
    let allowed = config.allowed_ips.clone();
    let network = thread::Builder::new()
        .name("video-listener".into())
        .spawn(move || ingress(video, state, secret, allowed))?;
    let local = format!("127.0.0.1:{}", config.usb_port);
    let state = frames.clone();
    let status = path.with_extension("status.json");
    thread::Builder::new().name("uvc-maintenance".into()).spawn(move||{
  let mut last_error=String::new();let mut last_name_error=String::new();let mut named=false;
  let mut measurement=Instant::now();let mut received_before=0u64;let mut supplied_before=0u64;
  loop {
   if network.is_finished(){crate::diagnostics::event("video.listener.failed",serde_json::json!({"reason":"ingress thread exited"}));std::process::exit(1);}
   let attached=crate::vhci::imported_devices().and_then(|ports|{
    if ports.iter().any(|p|p.busid==uvc::BUSID && p.server.eq_ignore_ascii_case(&local)){Ok(())}else{crate::vhci::attach(&local,uvc::BUSID,false).map(|port|{crate::diagnostics::event("uvc.local.attached",serde_json::json!({"port":port}));})}
   });
   if let Err(error)=attached {if error.to_string()!=last_error {last_error=error.to_string();crate::diagnostics::event("uvc.local.attach.failed",serde_json::json!({"error":last_error}));}}else{last_error.clear();}
   if !named {
    let wanted=format!("USB\\VID_1209&PID_F002\\{}",state.config().serial);
    let result=crate::camera::list().and_then(|cameras|{
     let camera=cameras.into_iter().find(|c|c.identities.iter().any(|i|i.eq_ignore_ascii_case(&wanted))).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"emulated camera is still enumerating"))?;
     if camera.name!=state.config().name {crate::camera::set_emulated_name(&camera.path,&state.config().name)?;}
     Ok(())
    });
    match result {Ok(())=>{named=true;crate::diagnostics::event("uvc.name.applied",serde_json::json!({"name":state.config().name,"serial":state.config().serial}));},Err(error)=>{if error.to_string()!=last_name_error{last_name_error=error.to_string();crate::diagnostics::event("uvc.name.pending",serde_json::json!({"error":last_name_error}));}}}
   }
   let received=state.received.load(Ordering::Relaxed);let supplied=state.supplied.load(Ordering::Relaxed);let elapsed=measurement.elapsed().as_secs_f64().max(0.001);
   let _=crate::settings::save(&status,&serde_json::json!({"process_id":std::process::id(),"running":true,"device_attached":last_error.is_empty(),"name_applied":named,"attach_error":last_error,"updated_unix_ms":crate::recovery::timestamp(),"source_connected":state.source_live(),"received_frames":received,"supplied_frames":supplied,"source_fps":(received-received_before)as f64/elapsed,"camera_fps":(supplied-supplied_before)as f64/elapsed,"camera":state.config()}));
   measurement=Instant::now();received_before=received;supplied_before=supplied;
   thread::sleep(Duration::from_secs(1));
  }
 })?;
    crate::diagnostics::event(
        "video.receiver.start",
        serde_json::json!({"camera":config.camera,"usb_port":config.usb_port,"video_port":config.video_port}),
    );
    Arc::new(crate::server::Server::new(
        Arc::new(UvcProvider::new(frames)),
        vec![uvc::BUSID.into()],
        vec![],
    ))
    .serve(usb)
}
fn send_frame(
    stream: &mut TcpStream,
    session: &[u8; 32],
    sequence: u64,
    jpeg: &[u8],
) -> io::Result<()> {
    let mut header = [0; 44];
    header[..8].copy_from_slice(&sequence.to_be_bytes());
    header[8..12].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
    let tag = mac(session, &[&header[..12], jpeg]).finalize().into_bytes();
    header[12..].copy_from_slice(&tag);
    let mut slices = [io::IoSlice::new(&header), io::IoSlice::new(jpeg)];
    wire::write_vectored_all(stream, &mut slices)
}
fn connect(
    config: &SenderSettings,
    secret: &[u8; 32],
) -> io::Result<(TcpStream, Config, [u8; 32])> {
    use std::net::ToSocketAddrs;
    let mut stream = config
        .server
        .to_socket_addrs()?
        .find_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(5)).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "video receiver unavailable",
            )
        })?;
    configure_socket(&stream)?;
    let mut h = [0; 48];
    read_exact(&mut stream, &mut h)?;
    if &h[..8] != MAGIC {
        return Err(wire::invalid("invalid video receiver handshake"));
    }
    let mode = Config {
        width: u16::from_be_bytes(h[8..10].try_into().unwrap()),
        height: u16::from_be_bytes(h[10..12].try_into().unwrap()),
        fps: u32::from_be_bytes(h[12..16].try_into().unwrap()),
        ..Config::default()
    };
    mode.validate()?;
    let signature = mac(secret, &[b"USBBridge video authenticate", &h])
        .finalize()
        .into_bytes();
    stream.write_all(&signature)?;
    let mut ok = [0; 4];
    read_exact(&mut stream, &mut ok)?;
    if &ok != b"OKAY" {
        return Err(wire::invalid("video receiver rejected connection"));
    }
    let mut proof = [0; 32];
    read_exact(&mut stream, &mut proof)?;
    mac(secret, &[b"USBBridge video receiver", &h, &signature])
        .verify_slice(&proof)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "video receiver authentication failed",
            )
        })?;
    Ok((stream, mode, session_key(secret, &h)))
}
#[cfg(windows)]
fn resize(frame: &crate::camera::Frame, width: u16, height: u16) -> Vec<u8> {
    let (w, h) = (usize::from(width), usize::from(height));
    let (sw, sh) = (frame.width as usize, frame.height as usize);
    let mut pixels = vec![0; w * h * 4];
    let (ox, oy, dw, dh) = crate::camera::fit(
        frame.width,
        frame.height,
        i32::from(width),
        i32::from(height),
    );
    if w == sw && h == sh {
        for (target, source) in pixels
            .chunks_exact_mut(w * 4)
            .zip(frame.pixels.rchunks_exact(w * 4))
        {
            target.copy_from_slice(source);
        }
        return pixels;
    }
    for y in 0..dh as usize {
        let sy = sh - 1 - y * sh / dh as usize;
        for x in 0..dw as usize {
            let sx = x * sw / dw as usize;
            let from = (sy * sw + sx) * 4;
            let to = ((y + oy as usize) * w + x + ox as usize) * 4;
            pixels[to..to + 4].copy_from_slice(&frame.pixels[from..from + 4]);
        }
    }
    pixels
}
#[cfg(windows)]
fn transmit(config: &SenderSettings, secret: &[u8; 32], path: &Path) -> io::Result<()> {
    let (mut stream, mode, session) = connect(config, secret)?;
    let period = Duration::from_nanos(u64::from(mode.interval()) * 100);
    let mut due = Instant::now();
    let worker = if config.camera == "test-pattern" {
        None
    } else {
        let cameras = crate::camera::list()?;
        let selected: Vec<_> = cameras
            .into_iter()
            .filter(|c| c.matches(&config.camera))
            .collect();
        let [camera] = selected.as_slice() else {
            return Err(wire::invalid(
                "video source not found; select its exact identity",
            ));
        };
        let modes = crate::camera::modes(&camera.path)?;
        let selected = modes
            .iter()
            .min_by_key(|m| {
                (
                    (m.width - i32::from(mode.width)).abs()
                        + (m.height - i32::from(mode.height)).abs(),
                    ((m.fps() - f64::from(mode.fps)).abs() * 1000.) as u64,
                    if m.format == "MJPEG" { 0 } else { 1 },
                )
            })
            .ok_or_else(|| wire::invalid("video source has no supported modes"))?;
        let worker = crate::camera::Worker::new()?;
        worker.send(crate::camera::Action::Start(
            camera.clone(),
            selected.clone(),
            false,
        ))?;
        Some(worker)
    };
    let mut sequence = 0u64;
    let mut source_sequence = 0u64;
    let mut last_status = Instant::now();
    let mut source_wait = Instant::now();
    crate::diagnostics::event(
        "video.sender.connected",
        serde_json::json!({"server":config.server,"source":config.camera,"width":mode.width,"height":mode.height,"fps":mode.fps}),
    );
    loop {
        if Instant::now() < due {
            thread::sleep(
                due.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(2)),
            );
            continue;
        }
        let jpeg = if let Some(worker) = &worker {
            let events = std::mem::take(&mut *worker.events.lock().unwrap());
            for event in events {
                if let crate::camera::Event::Error(error) = event {
                    return Err(io::Error::other(error));
                }
            }
            let frame = worker.frame.lock().unwrap().clone();
            let Some(frame) = frame.filter(|f| f.sequence != source_sequence) else {
                if source_wait.elapsed() > Duration::from_secs(10) {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "video source stopped producing frames",
                    ));
                }
                thread::sleep(Duration::from_millis(2));
                continue;
            };
            source_sequence = frame.sequence;
            source_wait = Instant::now();
            uvc::encode_bgra(
                &resize(&frame, mode.width, mode.height),
                mode.width,
                mode.height,
                config.quality,
            )?
        } else {
            uvc::pattern(&mode, sequence as usize % 12, false)?
        };
        if jpeg.len() > mode.frame_limit() {
            return Err(wire::invalid("encoded frame exceeds announced UVC size"));
        }
        sequence = sequence.saturating_add(1);
        send_frame(&mut stream, &session, sequence, &jpeg)?;
        let now = Instant::now();
        due = if now.saturating_duration_since(due) > period * 2 {
            now + period
        } else {
            due + period
        };
        if last_status.elapsed() >= Duration::from_secs(1) {
            crate::settings::save(
                &path.with_extension("status.json"),
                &serde_json::json!({"process_id":std::process::id(),"running":true,"connected":true,"updated_unix_ms":crate::recovery::timestamp(),"sent_frames":sequence,"width":mode.width,"height":mode.height,"fps":mode.fps}),
            )?;
            last_status = Instant::now();
        }
    }
}
#[cfg(windows)]
pub fn send(config: &SenderSettings, path: &Path) -> io::Result<()> {
    config.validate()?;
    if !config.enabled {
        return Ok(());
    }
    let _lease = crate::settings::acquire_file_lock(&path.with_extension("agent.lock"))?;
    crate::diagnostics::init(path, "video-sender");
    let secret = key(&config.key_file)?;
    let mut retry = 1u64;
    loop {
        if let Err(error) = transmit(config, &secret, path) {
            crate::diagnostics::event(
                "video.sender.retry",
                serde_json::json!({"error":error.to_string(),"retry_seconds":retry}),
            );
            let _ = crate::settings::save(
                &path.with_extension("status.json"),
                &serde_json::json!({"process_id":std::process::id(),"running":true,"connected":false,"updated_unix_ms":crate::recovery::timestamp(),"error":error.to_string()}),
            );
        }
        thread::sleep(Duration::from_secs(retry));
        retry = (retry * 2).min(10);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn challenge_and_frame_authentication_are_bound_to_session_and_contents() {
        let secret = [7; 32];
        let hello1 = hello(&Config::default());
        let hello2 = hello(&Config::default());
        assert_ne!(hello1, hello2);
        let key1 = session_key(&secret, &hello1);
        let key2 = session_key(&secret, &hello2);
        let signature = mac(&key1, &[b"sequence", b"frame"]).finalize().into_bytes();
        assert!(
            mac(&key1, &[b"sequence", b"frame"])
                .verify_slice(&signature)
                .is_ok()
        );
        assert!(
            mac(&key1, &[b"sequence", b"changed"])
                .verify_slice(&signature)
                .is_err()
        );
        assert!(
            mac(&key2, &[b"sequence", b"frame"])
                .verify_slice(&signature)
                .is_err()
        );
    }
    #[test]
    fn receiver_rejects_port_collision_and_missing_allowlist() {
        let config = ReceiverSettings {
            enabled: true,
            camera: Config::default(),
            usb_port: 13250,
            video_port: 13250,
            allowed_ips: vec![],
            key_file: "C:\\key.credential".into(),
        };
        assert!(config.validate().is_err());
    }
    #[test]
    fn ingress_authenticates_frames_and_rejects_a_replayed_sequence() {
        let frames = Frames::new(Config {
            width: 64,
            height: 48,
            test_pattern: false,
            ..Config::default()
        })
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let source = frames.clone();
        let active = Arc::new(AtomicBool::new(false));
        let owned = active.clone();
        let secret = [9; 32];
        let worker =
            thread::spawn(move || ingest(listener.accept().unwrap().0, source, &secret, owned));
        let mut stream = TcpStream::connect(address).unwrap();
        configure_socket(&stream).unwrap();
        let mut greeting = [0; 48];
        read_exact(&mut stream, &mut greeting).unwrap();
        stream
            .write_all(
                &mac(&secret, &[b"USBBridge video authenticate", &greeting])
                    .finalize()
                    .into_bytes(),
            )
            .unwrap();
        let mut ok = [0; 4];
        read_exact(&mut stream, &mut ok).unwrap();
        assert_eq!(&ok, b"OKAY");
        let session = session_key(&secret, &greeting);
        let jpeg = uvc::pattern(frames.config(), 0, false).unwrap();
        send_frame(&mut stream, &session, 1, &jpeg).unwrap();
        send_frame(&mut stream, &session, 1, &jpeg).unwrap();
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("sequence")
        );
        assert_eq!(frames.received.load(Ordering::Relaxed), 1);
        assert!(!active.load(Ordering::Acquire));
        assert!(!frames.source_live());
    }
    #[test]
    fn unauthenticated_sender_cannot_take_ownership_or_publish_an_image() {
        let frames = Frames::new(Config {
            width: 64,
            height: 48,
            test_pattern: false,
            ..Config::default()
        })
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let source = frames.clone();
        let owned = Arc::new(AtomicBool::new(false));
        let active = owned.clone();
        let worker =
            thread::spawn(move || ingest(listener.accept().unwrap().0, source, &[9; 32], active));
        let mut stream = TcpStream::connect(address).unwrap();
        let mut greeting = [0; 48];
        read_exact(&mut stream, &mut greeting).unwrap();
        stream.write_all(&[0; 32]).unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(!owned.load(Ordering::Acquire));
        assert_eq!(frames.received.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn stale_video_report_never_claims_a_live_source_or_camera() {
        let mut value = serde_json::json!({"running":true,"connected":true,"source_connected":true,"device_attached":true,"updated_unix_ms":1000,"received_frames":300});
        assert!(!expire_status(&mut value, 7000));
        assert_eq!(value["running"], false);
        assert_eq!(value["source_connected"], false);
        assert_eq!(value["device_attached"], false);
        assert_eq!(value["received_frames"], 300);
    }
    #[test]
    fn sender_rejects_a_receiver_without_the_shared_key_before_capture() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&hello(&Config::default())).unwrap();
            let mut signature = [0; 32];
            read_exact(&mut stream, &mut signature).unwrap();
            stream.write_all(b"OKAY").unwrap();
            stream.write_all(&[0; 32]).unwrap();
        });
        let config = SenderSettings {
            enabled: true,
            server: address.to_string(),
            camera: "test-pattern".into(),
            quality: 85,
            key_file: "unused.key".into(),
        };
        assert_eq!(
            connect(&config, &[9; 32]).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        worker.join().unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn dib_rows_are_upright_in_network_video_and_letterboxing_preserves_aspect() {
        let frame = crate::camera::Frame {
            width: 2,
            height: 2,
            sequence: 1,
            pixels: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
            ],
        };
        let output = resize(&frame, 2, 2);
        assert_eq!(&output[..4], &[0, 0, 255, 255]);
        assert_eq!(&output[8..12], &[255, 0, 0, 255]);
        let wide = resize(&frame, 4, 2);
        assert_eq!(&wide[..4], &[0; 4]);
        assert_eq!(&wide[4..8], &[0, 0, 255, 255]);
        assert_eq!(&wide[12..16], &[0; 4]);
    }
    #[test]
    fn gui_and_cli_use_the_registered_video_config_basename() {
        assert_eq!(
            receiver_path(Path::new("fixture/host.local.json")),
            Path::new("fixture/host.video-receive.local.json")
        );
        assert_eq!(
            sender_path(Path::new("fixture/client.local.json")),
            Path::new("fixture/client.video-send.local.json")
        );
    }
}
