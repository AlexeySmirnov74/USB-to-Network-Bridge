//! Byte-based backpressure retained until a completed transfer is written out.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
pub struct MemoryLease {
    used: Arc<AtomicUsize>,
    bytes: usize,
}
impl MemoryLease {
    pub fn reserve(used: &Arc<AtomicUsize>, bytes: usize, limit: usize) -> Option<Self> {
        let mut current = used.load(Ordering::Relaxed);
        loop {
            let next = current.checked_add(bytes).filter(|v| *v <= limit)?;
            match used.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => break,
                Err(value) => current = value,
            }
        }
        Some(Self {
            used: used.clone(),
            bytes,
        })
    }
}
impl Drop for MemoryLease {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
