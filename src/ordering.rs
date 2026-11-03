//! Keep transfer replies ordered per endpoint without serialising USB I/O.
use std::{
    collections::{HashMap, VecDeque},
    io,
};

pub(crate) struct ReplyOrder<T> {
    endpoints: HashMap<u8, VecDeque<u32>>,
    pending: HashMap<u32, u8>,
    ready: HashMap<u32, T>,
}
impl<T> Default for ReplyOrder<T> {
    fn default() -> Self {
        Self {
            endpoints: HashMap::new(),
            pending: HashMap::new(),
            ready: HashMap::new(),
        }
    }
}
impl<T> ReplyOrder<T> {
    pub(crate) fn register(&mut self, endpoint: u8, seq: u32) -> io::Result<()> {
        if self.pending.len() >= 4096 {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "too many queued endpoint replies",
            ));
        }
        if self.pending.contains_key(&seq) {
            return Err(crate::wire::invalid("duplicate pending request sequence"));
        }
        self.pending.insert(seq, endpoint);
        self.endpoints.entry(endpoint).or_default().push_back(seq);
        Ok(())
    }
    pub(crate) fn complete(&mut self, seq: u32, value: T) -> Vec<T> {
        let endpoint = self.pending[&seq];
        self.ready.insert(seq, value);
        let queue = self.endpoints.get_mut(&endpoint).unwrap();
        let mut output = Vec::new();
        while let Some(&first) = queue.front() {
            let Some(value) = self.ready.remove(&first) else {
                break;
            };
            queue.pop_front();
            self.pending.remove(&first);
            output.push(value);
        }
        if queue.is_empty() {
            self.endpoints.remove(&endpoint);
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn out_of_order_io_does_not_reorder_endpoint_or_block_other_endpoints() {
        let mut order = ReplyOrder::default();
        order.register(0x81, 9).unwrap();
        order.register(0x81, 2).unwrap();
        order.register(0x82, 5).unwrap();
        assert!(order.complete(2, "second").is_empty());
        assert_eq!(order.complete(5, "independent"), vec!["independent"]);
        assert_eq!(order.complete(9, "first"), vec!["first", "second"]);
        assert!(order.pending.is_empty());
        assert!(order.endpoints.is_empty());
    }
    #[test]
    fn duplicate_sequence_remains_reserved_until_reply_is_released() {
        let mut order = ReplyOrder::default();
        order.register(1, 1).unwrap();
        order.register(1, 2).unwrap();
        order.complete(2, ());
        assert!(order.register(2, 2).is_err());
        order.complete(1, ());
        order.register(2, 2).unwrap();
        assert_eq!(order.complete(2, ()), vec![()]);
    }
    #[test]
    fn stalled_endpoint_cannot_accumulate_unbounded_ready_replies() {
        let mut order = ReplyOrder::default();
        order.register(1, 0).unwrap();
        for seq in 1..4096 {
            order.register(1, seq).unwrap();
            assert!(order.complete(seq, ()).is_empty());
        }
        assert_eq!(
            order.register(2, 5000).unwrap_err().kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(order.complete(0, ()).len(), 4096);
        order.register(2, 5000).unwrap();
    }
}
