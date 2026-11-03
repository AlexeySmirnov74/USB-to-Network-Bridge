use crate::wire::{Completion, Device, Submit};
use std::{
    io,
    sync::{Arc, Mutex, mpsc::SyncSender},
};

pub trait Session: Send + Sync {
    fn connected(&self) -> bool {
        true
    }
    fn submit(&self, request: Submit) -> io::Result<()>;
    fn cancel(&self, seq: u32, target: u32) -> io::Result<()>;
}
pub trait Provider: Send + Sync {
    fn devices(&self) -> io::Result<Vec<Device>>;
    fn open(&self, busid: &str, completed: SyncSender<Completion>) -> io::Result<Arc<dyn Session>>;
}

/// A clearly identified synthetic bulk loopback device for protocol tests and benchmarks.
pub struct MockProvider;
pub fn mock_device() -> Device {
    Device {
        path: "/usbbridge/test-only/loopback".into(),
        busid: "test-1".into(),
        bus: 1,
        address: 1,
        speed: 3,
        vendor: 0x1209,
        product: 0xf001,
        revision: 0x0100,
        class: 0,
        subclass: 0,
        protocol: 0,
        configuration: 1,
        configurations: 1,
        interfaces: vec![[0xff, 0, 0, 0]],
    }
}
impl Provider for MockProvider {
    fn devices(&self) -> io::Result<Vec<Device>> {
        Ok(vec![mock_device()])
    }
    fn open(&self, busid: &str, completed: SyncSender<Completion>) -> io::Result<Arc<dyn Session>> {
        if busid != "test-1" {
            return Err(io::ErrorKind::NotFound.into());
        }
        Ok(Arc::new(MockSession {
            completed,
            last: Mutex::new(Vec::new()),
        }))
    }
}
struct MockSession {
    completed: SyncSender<Completion>,
    last: Mutex<Vec<u8>>,
}
impl Session for MockSession {
    fn submit(&self, mut request: Submit) -> io::Result<()> {
        let mut data = Vec::new();
        let mut status = 0;
        let mut actual = request.length;
        if request.endpoint == 0 {
            match request.setup[1] {
                6 => {
                    let source: &[u8] = match request.setup[3] {
                        1 => &[18, 1, 0, 2, 0, 0, 0, 64, 9, 0x12, 1, 0xf0, 0, 1, 0, 0, 0, 1],
                        2 => &[
                            9, 2, 32, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 2, 0xff, 0, 0, 0, 7, 5, 1,
                            2, 0, 2, 0, 7, 5, 0x81, 2, 0, 2, 0,
                        ],
                        _ => &[],
                    };
                    data.extend_from_slice(&source[..source.len().min(request.length)]);
                    actual = data.len();
                }
                9 | 11 => actual = 0,
                _ => {
                    status = -32;
                    actual = 0;
                }
            }
        } else if request.direction == 0 {
            *self.last.lock().unwrap() = std::mem::take(&mut request.data);
        } else {
            let last = self.last.lock().unwrap();
            data.resize(request.length, 0);
            if !last.is_empty() {
                for chunk in data.chunks_mut(last.len()) {
                    chunk.copy_from_slice(&last[..chunk.len()]);
                }
            }
        }
        let iso = request
            .iso
            .iter()
            .map(|p| crate::wire::IsoPacket {
                actual: p.length,
                ..p.clone()
            })
            .collect();
        self.completed
            .send(Completion::submit(&request, status, actual, data, 0, iso))
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
    fn cancel(&self, seq: u32, _target: u32) -> io::Result<()> {
        self.completed
            .send(Completion::unlink(seq, 0))
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
}
