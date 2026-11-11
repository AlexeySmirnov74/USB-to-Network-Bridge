//! A software UVC 1.1 MJPEG device carried by the existing USB/IP transport.
//! Protocol layouts follow USB Video Class; no physical device is claimed.
use crate::{
    backend::{Provider, Session},
    wire::{self, Completion, Device, Submit},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

pub const BUSID: &str = "camera-1";
/// This software endpoint must never be recaptured by the physical USB backend.
pub fn is_emulated_identity(identity: &str) -> bool {
    let value = identity.to_ascii_uppercase();
    value.starts_with("USB\\VID_1209&PID_F002\\") || value.starts_with("USB\\VID_1209&PID_F002&MI_")
}

pub const MAX_JPEG: usize = 16 * 1024 * 1024;
const PAYLOAD: usize = 16 * 1024;
const CLOCK: u32 = 48_000_000;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub name: String,
    pub serial: String,
    pub width: u16,
    pub height: u16,
    pub fps: u32,
    pub test_pattern: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            name: "4k web camera".into(),
            serial: "USBBridge-Camera-1".into(),
            width: 640,
            height: 480,
            fps: 30,
            test_pattern: true,
        }
    }
}
impl Config {
    pub fn validate(&self) -> io::Result<()> {
        if self.name.trim().is_empty()
            || self.name.encode_utf16().count() > 100
            || self.name.chars().any(char::is_control)
            || self.serial.is_empty()
            || self.serial.len() > 100
            || !self.serial.is_ascii()
            || self.serial.chars().any(char::is_control)
        {
            return Err(wire::invalid("camera name/serial is invalid"));
        }
        if !(16..=3840).contains(&self.width)
            || !(16..=2160).contains(&self.height)
            || !self.width.is_multiple_of(2)
            || !(1..=60).contains(&self.fps)
        {
            return Err(wire::invalid(
                "camera mode must be even width 16..3840, height 16..2160 and 1..60 FPS",
            ));
        }
        Ok(())
    }
    pub fn interval(&self) -> u32 {
        10_000_000 / self.fps
    }
    pub fn frame_limit(&self) -> usize {
        (usize::from(self.width) * usize::from(self.height) * 3 + 65536).min(MAX_JPEG)
    }
    fn device(&self) -> Device {
        Device {
            path: format!("/usbbridge/uvc/{}", self.serial),
            busid: BUSID.into(),
            bus: 7,
            address: 1,
            speed: 3,
            vendor: 0x1209,
            product: 0xf002,
            revision: 0x0100,
            class: 0xef,
            subclass: 2,
            protocol: 1,
            configuration: 1,
            configurations: 1,
            interfaces: vec![[14, 1, 0, 0], [14, 2, 0, 0]],
        }
    }
    pub fn device_descriptor(&self) -> Vec<u8> {
        vec![
            18, 1, 0, 2, 0xef, 2, 1, 64, 9, 0x12, 2, 0xf0, 0, 1, 1, 2, 3, 1,
        ]
    }
    pub fn configuration_descriptor(&self) -> Vec<u8> {
        let mut vc = vec![13, 0x24, 1, 0x10, 1, 40, 0];
        vc.extend(CLOCK.to_le_bytes());
        vc.extend([1, 1]);
        vc.extend([18, 0x24, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0]);
        vc.extend([9, 0x24, 3, 2, 1, 1, 0, 1, 0]);
        let mut vs = vec![14, 0x24, 1, 1, 61, 0, 0x81, 0, 2, 0, 0, 0, 1, 0];
        vs.extend([11, 0x24, 6, 1, 1, 1, 1, 0, 0, 0, 0]);
        vs.extend([30, 0x24, 7, 1, 0]);
        vs.extend(self.width.to_le_bytes());
        vs.extend(self.height.to_le_bytes());
        let bitrate = (self.frame_limit() as u32)
            .saturating_mul(8)
            .saturating_mul(self.fps);
        vs.extend(bitrate.to_le_bytes());
        vs.extend(bitrate.to_le_bytes());
        vs.extend((self.frame_limit() as u32).to_le_bytes());
        vs.extend(self.interval().to_le_bytes());
        vs.push(1);
        vs.extend(self.interval().to_le_bytes());
        vs.extend([6, 0x24, 0x0d, 1, 1, 4]);
        let length = 9 + 8 + 9 + vc.len() + 9 + vs.len() + 7;
        let mut result = vec![9, 2];
        result.extend((length as u16).to_le_bytes());
        result.extend([2, 1, 0, 0x80, 50]);
        result.extend([8, 11, 0, 2, 14, 3, 0, 2]);
        result.extend([9, 4, 0, 0, 0, 14, 1, 0, 0]);
        result.extend(vc);
        result.extend([9, 4, 1, 0, 1, 14, 2, 0, 0]);
        result.extend(vs);
        result.extend([7, 5, 0x81, 2, 0, 2, 0]);
        result
    }
    fn probe(&self) -> Vec<u8> {
        let mut result = vec![0, 0, 1, 1];
        result.extend(self.interval().to_le_bytes());
        result.extend([0; 10]);
        result.extend((self.frame_limit() as u32).to_le_bytes());
        result.extend((PAYLOAD as u32).to_le_bytes());
        result.extend(CLOCK.to_le_bytes());
        result.extend([3, 1, 1, 1]);
        result
    }
    fn string_descriptor(&self, index: u8) -> Option<Vec<u8>> {
        if index == 0 {
            return Some(vec![4, 3, 9, 4]);
        }
        let value = match index {
            1 => "USB to Network Bridge",
            2 => &self.name,
            3 => &self.serial,
            _ => return None,
        };
        let mut result = vec![0, 3];
        for word in value.encode_utf16() {
            result.extend(word.to_le_bytes());
        }
        result[0] = result.len() as u8;
        Some(result)
    }
}

/// Only baseline JPEG of the announced size is accepted before kernel delivery.
pub fn jpeg_size(bytes: &[u8]) -> io::Result<(u16, u16)> {
    if bytes.len() < 4 || bytes[..2] != [0xff, 0xd8] || bytes[bytes.len() - 2..] != [0xff, 0xd9] {
        return Err(wire::invalid("invalid JPEG envelope"));
    }
    let mut pos = 2;
    while pos + 4 <= bytes.len() && pos < 1024 * 1024 {
        if bytes[pos] != 0xff {
            return Err(wire::invalid("invalid JPEG marker"));
        }
        while pos < bytes.len() && bytes[pos] == 0xff {
            pos += 1;
        }
        let marker = *bytes
            .get(pos)
            .ok_or_else(|| wire::invalid("truncated JPEG marker"))?;
        pos += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let size = bytes
            .get(pos..pos + 2)
            .ok_or_else(|| wire::invalid("truncated JPEG length"))?;
        let length = usize::from(u16::from_be_bytes([size[0], size[1]]));
        if length < 2 || pos + length > bytes.len() {
            return Err(wire::invalid("JPEG segment outside frame"));
        }
        if marker == 0xc0 {
            if length < 8 || bytes[pos + 2] != 8 {
                return Err(wire::invalid("unsupported JPEG precision"));
            }
            return Ok((
                u16::from_be_bytes([bytes[pos + 5], bytes[pos + 6]]),
                u16::from_be_bytes([bytes[pos + 3], bytes[pos + 4]]),
            ));
        }
        if (0xc1..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
            return Err(wire::invalid("UVC input requires baseline JPEG"));
        }
        pos += length;
    }
    Err(wire::invalid("JPEG has no baseline frame header"))
}
/// A reused output buffer with a hard bound enforced while the encoder writes.
struct JpegOutput<'a> {
    bytes: &'a mut Vec<u8>,
    limit: usize,
}
impl io::Write for JpegOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| wire::invalid("encoded frame exceeds announced UVC size"))?;
        if length > self.bytes.capacity() {
            let capacity = length
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limit);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(crate) fn encode_bgra_into(
    pixels: &[u8],
    width: u16,
    height: u16,
    quality: u8,
    output: &mut Vec<u8>,
    limit: usize,
) -> io::Result<()> {
    if width == 0
        || height == 0
        || pixels.len() != usize::from(width) * usize::from(height) * 4
        || !(1..=100).contains(&quality)
        || limit == 0
        || limit > MAX_JPEG
    {
        return Err(wire::invalid("invalid JPEG image dimensions/quality/limit"));
    }
    output.clear();
    let result = jpeg_encoder::Encoder::new(
        JpegOutput {
            bytes: output,
            limit,
        },
        quality,
    )
    .encode(pixels, width, height, jpeg_encoder::ColorType::Bgra)
    .map_err(io::Error::other);
    if result.is_err() {
        output.clear();
    }
    result
}
pub fn encode_bgra(pixels: &[u8], width: u16, height: u16, quality: u8) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    encode_bgra_into(pixels, width, height, quality, &mut result, MAX_JPEG)?;
    Ok(result)
}
pub fn pattern(config: &Config, phase: usize, black: bool) -> io::Result<Vec<u8>> {
    let w = usize::from(config.width);
    let h = usize::from(config.height);
    let mut pixels = vec![0; w * h * 4];
    let bars = [
        [235, 235, 235],
        [16, 235, 235],
        [235, 235, 16],
        [16, 235, 16],
        [235, 16, 235],
        [16, 16, 235],
        [235, 16, 16],
        [16, 16, 16],
    ];
    if !black {
        for y in 0..h {
            for x in 0..w {
                let mut c = bars[(x * 8 / w).min(7)];
                if y > h * 3 / 4 {
                    c = if (x + phase * w / 12) % w < w / 12 {
                        [235; 3]
                    } else {
                        [16; 3]
                    };
                }
                let p = (y * w + x) * 4;
                pixels[p..p + 3].copy_from_slice(&c);
            }
        }
    }
    encode_bgra(&pixels, config.width, config.height, 80)
}
struct Latest {
    bytes: Arc<Vec<u8>>,
    received: Instant,
}
pub struct Frames {
    config: Config,
    latest: Mutex<Option<Latest>>,
    patterns: Vec<Arc<Vec<u8>>>,
    black: Arc<Vec<u8>>,
    pub received: AtomicU64,
    pub supplied: AtomicU64,
}
impl Frames {
    pub fn new(config: Config) -> io::Result<Arc<Self>> {
        config.validate()?;
        let black = Arc::new(pattern(&config, 0, true)?);
        let patterns = if config.test_pattern {
            (0..12)
                .map(|n| pattern(&config, n, false).map(Arc::new))
                .collect::<io::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Ok(Arc::new(Self {
            config,
            latest: Mutex::new(None),
            patterns,
            black,
            received: AtomicU64::new(0),
            supplied: AtomicU64::new(0),
        }))
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn accept(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.len() > self.config.frame_limit()
            || jpeg_size(&bytes)? != (self.config.width, self.config.height)
        {
            return Err(wire::invalid("JPEG mode/size differs from virtual camera"));
        }
        *self.latest.lock().unwrap() = Some(Latest {
            bytes: Arc::new(bytes),
            received: Instant::now(),
        });
        self.received.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    pub fn clear(&self) {
        *self.latest.lock().unwrap() = None;
    }
    pub fn source_live(&self) -> bool {
        self.latest
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|v| v.received.elapsed() < Duration::from_secs(2))
    }
    fn next(&self, index: usize) -> Arc<Vec<u8>> {
        if let Some(latest) = self
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .filter(|v| v.received.elapsed() < Duration::from_secs(2))
        {
            return latest.bytes.clone();
        }
        self.patterns
            .get(index % self.patterns.len().max(1))
            .unwrap_or(&self.black)
            .clone()
    }
}
pub struct UvcProvider {
    frames: Arc<Frames>,
}
impl UvcProvider {
    pub fn new(frames: Arc<Frames>) -> Self {
        Self { frames }
    }
}
impl Provider for UvcProvider {
    fn devices(&self) -> io::Result<Vec<Device>> {
        Ok(vec![self.frames.config.device()])
    }
    fn open(&self, busid: &str, completed: SyncSender<Completion>) -> io::Result<Arc<dyn Session>> {
        if busid != BUSID {
            return Err(io::ErrorKind::NotFound.into());
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pending: VecDeque::new(),
                active: None,
                cancel_active: false,
                committed: false,
            }),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let signal = shared.clone();
        let frames = self.frames.clone();
        let output = completed.clone();
        let worker = thread::Builder::new()
            .name("uvc-bulk".into())
            .spawn(move || stream_frames(signal, frames, output))?;
        Ok(Arc::new(UvcSession {
            shared,
            completed,
            config: self.frames.config.clone(),
            worker: Mutex::new(Some(worker)),
        }))
    }
}
struct State {
    pending: VecDeque<Submit>,
    active: Option<u32>,
    cancel_active: bool,
    committed: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    stop: AtomicBool,
}
struct UvcSession {
    shared: Arc<Shared>,
    completed: SyncSender<Completion>,
    config: Config,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}
fn deliver(sender: &SyncSender<Completion>, stop: &AtomicBool, mut reply: Completion) -> bool {
    while !stop.load(Ordering::Acquire) {
        match sender.try_send(reply) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(value)) => {
                reply = value;
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
    false
}
impl UvcSession {
    fn control(&self, request: &Submit) -> Option<Vec<u8>> {
        let s = request.setup;
        let value = u16::from_le_bytes([s[2], s[3]]);
        let index = u16::from_le_bytes([s[4], s[5]]);
        let result = match s[0] & 0x60 {
            0 => match s[1] {
                6 => match (value >> 8) as u8 {
                    1 => self.config.device_descriptor(),
                    2 => self.config.configuration_descriptor(),
                    3 => self.config.string_descriptor(value as u8)?,
                    6 => vec![10, 6, 0, 2, 0xef, 2, 1, 64, 1, 0],
                    7 => {
                        let mut d = self.config.configuration_descriptor();
                        d[1] = 7;
                        let n = d.len();
                        d[n - 3..n - 1].copy_from_slice(&64u16.to_le_bytes());
                        d
                    }
                    _ => return None,
                },
                0 => vec![0, 0],
                8 => vec![1],
                9 if value <= 1 => Vec::new(),
                10 if index <= 1 => vec![0],
                11 if value == 0 && index <= 1 => Vec::new(),
                1 if s[0] & 0x1f == 2 => Vec::new(),
                _ => return None,
            },
            0x20 if index == 1 && [1, 2].contains(&((value >> 8) as u8)) => match s[1] {
                1 => {
                    if request.direction != 0
                        || request.data.len() < 26
                        || request.data[2] != 1
                        || request.data[3] != 1
                    {
                        return None;
                    }
                    if value >> 8 == 2 {
                        self.shared.state.lock().unwrap().committed = true;
                        self.shared.wake.notify_all();
                    }
                    Vec::new()
                }
                0x81..=0x84 | 0x87 => self.config.probe(),
                0x85 => 34u16.to_le_bytes().to_vec(),
                0x86 => vec![3],
                _ => return None,
            },
            0x20 if index == 0 && value == 0x0200 => match s[1] {
                0x81 => vec![0],
                0x86 => vec![1],
                _ => return None,
            },
            0x20 if index == 1 && value == 0x0600 => match s[1] {
                0x81 => vec![0],
                0x86 => vec![1],
                _ => return None,
            },
            _ => return None,
        };
        Some(result)
    }
}
impl Session for UvcSession {
    fn submit(&self, request: Submit) -> io::Result<()> {
        if request.endpoint == 0 {
            let reply = self.control(&request);
            let ok = reply.is_some();
            let mut bytes = reply.unwrap_or_default();
            bytes.truncate(request.length.min(usize::from(u16::from_le_bytes([
                request.setup[6],
                request.setup[7],
            ]))));
            let actual = if request.direction == 0 && ok {
                request.data.len()
            } else {
                bytes.len()
            };
            crate::diagnostics::event(
                "uvc.control",
                serde_json::json!({"setup":request.setup,"status":if ok{0}else{-32},"actual":actual}),
            );
            if !deliver(
                &self.completed,
                &self.shared.stop,
                Completion::submit(
                    &request,
                    if ok { 0 } else { -32 },
                    actual,
                    bytes,
                    0,
                    Vec::new(),
                ),
            ) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
        } else if request.endpoint == 1
            && request.direction == 1
            && request.iso.is_empty()
            && request.length >= 6
            && request.length <= 1024 * 1024
        {
            let mut state = self.shared.state.lock().unwrap();
            if state.pending.len() >= 128 {
                return Err(io::Error::other("UVC request queue limit reached"));
            }
            state.pending.push_back(request);
            self.shared.wake.notify_all();
        } else if !deliver(
            &self.completed,
            &self.shared.stop,
            Completion::submit(&request, -32, 0, Vec::new(), 0, Vec::new()),
        ) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
    fn cancel(&self, seq: u32, target: u32) -> io::Result<()> {
        let mut state = self.shared.state.lock().unwrap();
        let before = state.pending.len();
        state.pending.retain(|r| r.seq != target);
        let found = before != state.pending.len() || state.active == Some(target);
        if state.active == Some(target) {
            state.cancel_active = true;
        }
        drop(state);
        if !deliver(
            &self.completed,
            &self.shared.stop,
            Completion::unlink(seq, if found { 0 } else { -2 }),
        ) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
}
impl Drop for UvcSession {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
fn stream_frames(shared: Arc<Shared>, frames: Arc<Frames>, completed: SyncSender<Completion>) {
    let start = Instant::now();
    let period = Duration::from_nanos(u64::from(frames.config.interval()) * 100);
    let mut due = Instant::now();
    let mut current = Arc::new(Vec::new());
    let mut offset = 0;
    let mut number = 0usize;
    let mut presentation = 0u32;
    loop {
        let request = {
            let mut state = shared.state.lock().unwrap();
            while (state.pending.is_empty() || !state.committed)
                && !shared.stop.load(Ordering::Acquire)
            {
                state = shared.wake.wait(state).unwrap();
            }
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            let r = state.pending.pop_front().unwrap();
            state.active = Some(r.seq);
            state.cancel_active = false;
            r
        };
        if offset == 0 {
            while Instant::now() < due && !shared.stop.load(Ordering::Acquire) {
                thread::sleep(
                    due.saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(2)),
                );
            }
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            current = frames.next(number);
            presentation = (start.elapsed().as_nanos() * u128::from(CLOCK) / 1_000_000_000) as u32;
            let now = Instant::now();
            due = if now.saturating_duration_since(due) > period * 2 {
                now + period
            } else {
                due + period
            };
        }
        let count = (request.length.min(PAYLOAD) - 6).min(current.len() - offset);
        let end = offset + count == current.len();
        let mut data = Vec::with_capacity(count + 6);
        data.extend([6, 0x84 | ((number & 1) as u8) | if end { 2 } else { 0 }]);
        data.extend(presentation.to_le_bytes());
        data.extend_from_slice(&current[offset..offset + count]);
        let cancelled = {
            let mut state = shared.state.lock().unwrap();
            let cancelled = state.cancel_active;
            state.active = None;
            cancelled
        };
        if !cancelled {
            offset += count;
            if end {
                offset = 0;
                number = number.wrapping_add(1);
                frames.supplied.fetch_add(1, Ordering::Relaxed);
            }
            let len = data.len();
            if !deliver(
                &completed,
                &shared.stop,
                Completion::submit(&request, 0, len, data, 0, Vec::new()),
            ) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jpeg_output_is_bounded_during_encoding_and_reuses_storage() {
        let pixels = vec![127; 64 * 64 * 4];
        let mut output = Vec::new();
        encode_bgra_into(&pixels, 64, 64, 85, &mut output, 4096).unwrap();
        assert_eq!(jpeg_size(&output).unwrap(), (64, 64));
        let pointer = output.as_ptr();
        let capacity = output.capacity();
        for _ in 0..40 {
            encode_bgra_into(&pixels, 64, 64, 85, &mut output, 4096).unwrap();
            assert_eq!(output.as_ptr(), pointer);
            assert_eq!(output.capacity(), capacity);
        }
        let mut small = Vec::new();
        assert!(encode_bgra_into(&pixels, 64, 64, 100, &mut small, 64).is_err());
        assert!(small.is_empty());
        assert!(small.capacity() <= 64);
        assert!(encode_bgra_into(&[], 0, 64, 85, &mut small, 4096).is_err());
    }
    #[test]
    fn descriptors_have_valid_lengths_and_exact_mode() {
        let c = Config::default();
        let d = c.configuration_descriptor();
        assert_eq!(usize::from(u16::from_le_bytes([d[2], d[3]])), d.len());
        let mut p = 0;
        while p < d.len() {
            let n = usize::from(d[p]);
            assert!(n >= 2 && p + n <= d.len());
            p += n;
        }
        assert_eq!(p, d.len());
        assert_eq!(c.probe().len(), 34);
        assert!(d.windows(4).any(|b| b == [0x80, 2, 0xe0, 1]));
    }
    #[test]
    fn jpeg_dimensions_are_checked_before_kernel_delivery() {
        let c = Config::default();
        let bytes = pattern(&c, 0, false).unwrap();
        assert_eq!(jpeg_size(&bytes).unwrap(), (640, 480));
        let frames = Frames::new(c).unwrap();
        frames.accept(bytes).unwrap();
        assert!(frames.source_live());
        let mut bad = pattern(
            &Config {
                width: 320,
                height: 240,
                ..Config::default()
            },
            0,
            false,
        )
        .unwrap();
        assert!(frames.accept(bad.clone()).is_err());
        bad.truncate(20);
        assert!(jpeg_size(&bad).is_err());
        frames.clear();
        assert!(!frames.source_live());
    }
    #[test]
    fn invalid_modes_names_and_progressive_jpeg_are_rejected() {
        let mut c = Config {
            width: 641,
            ..Config::default()
        };
        assert!(c.validate().is_err());
        c.width = 640;
        c.name = "bad\0name".into();
        assert!(c.validate().is_err());
        assert!(jpeg_size(&[0xff, 0xd8, 0xff, 0xc2, 0, 8, 8, 0, 1, 0, 1, 1, 0xff, 0xd9]).is_err());
    }
    fn request(
        seq: u32,
        endpoint: u32,
        direction: u32,
        length: usize,
        setup: [u8; 8],
        data: Vec<u8>,
    ) -> Submit {
        Submit {
            seq,
            devid: (7 << 16) | 1,
            endpoint,
            direction,
            length,
            setup,
            data,
            flags: 0,
            start_frame: 0,
            packet_count: 0,
            interval: 0,
            iso: Vec::new(),
        }
    }
    #[test]
    fn usb_frame_is_complete_jpeg_with_stable_timestamp_and_frame_boundary() {
        let frames = Frames::new(Config {
            width: 64,
            height: 48,
            ..Config::default()
        })
        .unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let session = UvcProvider::new(frames).open(BUSID, tx).unwrap();
        let mut probe = Config::default().probe();
        probe[2] = 1;
        probe[3] = 1;
        session
            .submit(request(1, 0, 0, 34, [0x21, 1, 0, 2, 1, 0, 34, 0], probe))
            .unwrap();
        assert_eq!(
            wire::u32be(&rx.recv_timeout(Duration::from_secs(1)).unwrap().header[20..]),
            0
        );
        let mut jpeg = Vec::new();
        let mut pts = None;
        let mut fid = None;
        for seq in 2..200 {
            session
                .submit(request(seq, 1, 1, 512, [0; 8], Vec::new()))
                .unwrap();
            let reply = rx.recv_timeout(Duration::from_secs(1)).unwrap();
            let data = &reply.data;
            assert_eq!(data[0], 6);
            assert_eq!(*pts.get_or_insert(data[2..6].to_vec()), data[2..6]);
            assert_eq!(*fid.get_or_insert(data[1] & 1), data[1] & 1);
            jpeg.extend_from_slice(&data[6..]);
            if data[1] & 2 != 0 {
                break;
            }
        }
        assert_eq!(jpeg_size(&jpeg).unwrap(), (64, 48));
        session
            .submit(request(201, 1, 1, PAYLOAD, [0; 8], Vec::new()))
            .unwrap();
        let next = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(next.data[1] & 1, fid.unwrap());
    }
    #[test]
    fn unlink_removes_queued_request_without_later_completion() {
        let frames = Frames::new(Config {
            width: 64,
            height: 48,
            ..Config::default()
        })
        .unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let session = UvcProvider::new(frames).open(BUSID, tx).unwrap();
        session
            .submit(request(1, 1, 1, 512, [0; 8], Vec::new()))
            .unwrap();
        session.cancel(2, 1).unwrap();
        let reply = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(wire::u32be(&reply.header), wire::RET_UNLINK);
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    }
    #[test]
    fn stale_network_image_is_replaced_by_black_without_removing_camera() {
        let frames = Frames::new(Config {
            width: 64,
            height: 48,
            test_pattern: false,
            ..Config::default()
        })
        .unwrap();
        let image = pattern(frames.config(), 0, false).unwrap();
        *frames.latest.lock().unwrap() = Some(Latest {
            bytes: Arc::new(image),
            received: Instant::now() - Duration::from_secs(3),
        });
        assert!(!frames.source_live());
        assert_eq!(*frames.next(0), *frames.black);
        assert_eq!(UvcProvider::new(frames).devices().unwrap().len(), 1);
    }
}
