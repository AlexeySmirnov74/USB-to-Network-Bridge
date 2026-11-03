//! Repeat complete protocol lifecycles; no hardware, persisted state or driver changes.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    thread,
};
use usbbridge::{backend::MockProvider, client, server::Server};

struct CountingAllocator;
static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
#[cfg(windows)]
fn handles() -> u32 {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut count = 0;
    assert_ne!(
        unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) },
        0
    );
    count
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            LIVE_BYTES.fetch_add(size as i64 - layout.size() as i64, Ordering::Relaxed);
        }
        result
    }
}
#[test]
fn repeated_bulk_sessions_release_payloads_threads_and_server_ownership() {
    const WARMUP: usize = 24;
    const CYCLES: usize = 200;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = Arc::new(Server::new(
        Arc::new(MockProvider),
        vec!["all".into()],
        Vec::new(),
    ));
    let accepted = server.clone();
    let (finished, idle) = std::sync::mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        for _ in 0..WARMUP + CYCLES {
            let (socket, _) = listener.accept().unwrap();
            let _ = accepted.connection(socket);
            finished.send(()).unwrap();
        }
    });
    let mut baseline = 0;
    let mut peak_retained = 0;
    #[cfg(windows)]
    let mut baseline_handles = 0;
    for cycle in 0..WARMUP + CYCLES {
        let result = client::bench(&address, 1, 65536, 8, cycle % 2 == 0).unwrap();
        assert_eq!(result.bytes, 1024 * 1024);
        idle.recv().unwrap();
        if cycle == WARMUP - 1 {
            baseline = LIVE_BYTES.load(Ordering::Relaxed);
            #[cfg(windows)]
            {
                baseline_handles = handles();
            }
        }
        if cycle >= WARMUP {
            peak_retained = peak_retained.max(LIVE_BYTES.load(Ordering::Relaxed) - baseline);
        }
    }
    worker.join().unwrap();
    assert_eq!(
        Arc::strong_count(&server),
        1,
        "connection retained the server after teardown"
    );
    let retained = LIVE_BYTES.load(Ordering::Relaxed) - baseline;
    #[cfg(windows)]
    {
        let final_handles = handles();
        assert!(
            final_handles <= baseline_handles + 2,
            "handles grew from {baseline_handles} to {final_handles}"
        );
        println!("memory lifecycle handles: baseline={baseline_handles} final={final_handles}");
    }
    // Permit test-runner output and lazy standard-library caches, but detect
    // even one retained bulk buffer per lifecycle (200 * 64 KiB).
    assert!(
        retained < 256 * 1024,
        "retained {retained} Rust heap bytes after {CYCLES} sessions"
    );
    println!(
        "memory lifecycle: warmup={WARMUP} sessions={CYCLES} transfer_mib={CYCLES} retained_rust_bytes={retained} peak_after_warmup_bytes={peak_retained}"
    );
}
