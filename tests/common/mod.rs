//! Shared test helpers and harness.

/// Deterministic 64-bit XorShift pseudorandom number generator strictly
/// conforming to AGENTS.md rule 11.1.
#[derive(Debug, Clone)]
pub struct XorShift64 {
    state: u64,
}

#[allow(dead_code)]
impl XorShift64 {
    /// Creates a new generator with the given non-zero seed.
    ///
    /// # Panics
    ///
    /// Panics if `seed == 0` (zero state is prohibited in XorShift).
    pub fn new(seed: u64) -> Self {
        assert_ne!(seed, 0, "XorShift64 seed must not be zero");
        Self { state: seed }
    }

    /// Emits the next pseudorandom 64-bit word.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Returns a pseudorandom integer in half-open range `[min, max)`.
    ///
    /// Extracts high-order bits (`>> 32`) and computes modulo on wide `u64`
    /// before casting to `usize`. Note: accepts `% diff` modulo bias for performance.
    ///
    /// # Panics
    ///
    /// Panics if `min >= max`.
    pub fn gen_range(&mut self, min: usize, max: usize) -> usize {
        assert!(min < max, "empty range: min must be strictly less than max");
        let diff = (max - min) as u64;
        let offset = ((self.next_u64() >> 32) % diff) as usize;
        min + offset
    }

    /// Returns a pseudorandom boolean by sampling the most significant bit (MSB).
    // Allow dead_code because common is included across independent integration test targets.
    #[allow(dead_code)]
    pub fn gen_bool(&mut self) -> bool {
        (self.next_u64() >> 63) == 1
    }
}

/// Managed test server running in a background thread for end-to-end TCP testing.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub struct TestServer {
    addr: std::net::SocketAddr,
    shutdown_handle: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

#[cfg(target_os = "linux")]
#[allow(dead_code)]
impl TestServer {
    /// Starts a test server backed by default engine on an ephemeral localhost port.
    #[must_use]
    pub fn start() -> Self {
        Self::start_with_engine(cachedb::cmd::Engine::new())
    }

    /// Starts a test server backed by a custom engine on an ephemeral localhost port.
    #[must_use]
    pub fn start_with_engine(engine: cachedb::cmd::Engine) -> Self {
        use cachedb::net::event_loop::EventLoop;
        use cachedb::net::sys::{EpollFlags, as_raw_fd};
        use cachedb::server::{LISTENER_TOKEN, Server};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let server = Server::with_engine(listener, engine).expect("create server");

        let mut event_loop = EventLoop::new().expect("create event loop");
        event_loop
            .register(
                as_raw_fd(server.listener()),
                LISTENER_TOKEN,
                EpollFlags::READABLE,
            )
            .expect("register listener");

        let shutdown_handle = event_loop.shutdown_handle();
        let mut server = server;

        let thread_handle = std::thread::spawn(move || event_loop.run(&mut server));

        Self {
            addr,
            shutdown_handle,
            thread_handle: Some(thread_handle),
        }
    }

    /// Returns the local socket address of the test server.
    #[must_use]
    pub const fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Connects a new client stream with 5-second read and write timeouts.
    #[must_use]
    pub fn connect(&self) -> std::net::TcpStream {
        let stream = std::net::TcpStream::connect(self.addr).expect("connect to test server");
        let timeout = std::time::Duration::from_secs(5);
        stream
            .set_read_timeout(Some(timeout))
            .expect("set read timeout");
        stream
            .set_write_timeout(Some(timeout))
            .expect("set write timeout");
        stream
    }

    /// Requests shutdown of the server and waits for the thread to exit.
    pub fn stop(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            self.shutdown_handle
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = std::net::TcpStream::connect(self.addr);
            let _ = handle.join();
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}
