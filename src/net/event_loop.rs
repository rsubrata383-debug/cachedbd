//! Non-blocking I/O event loop abstraction.
//!
//! This module provides a single-threaded event loop driven by operating system
//! notifications via [`crate::net::sys::Epoll`]. It dispatches socket readiness
//! events and scheduled monotonic timers to an application [`Handler`] without
//! heap closures, locks, or reference counting overhead.

use crate::net::sys::{Epoll, EpollFlags, Event, RawFd, is_shutdown_requested};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::io;
use std::time::{Duration, Instant};

/// User-defined identifier associated with a registered file descriptor.
pub type Token = u64;

/// Identifier associated with a scheduled timer event.
pub type TimerId = u64;

/// Application event handler driven by the [`EventLoop`].
///
/// Handlers are called sequentially on the single main thread. Because all calls
/// are synchronous and single-threaded, handlers do not require synchronization
/// primitives such as `Mutex`, `RwLock`, or `Arc`.
///
/// # Error Handling Policy
///
/// Returning an `Err` from [`Handler::on_event`] or [`Handler::on_timeout`] propagates
/// immediately out of [`EventLoop::run_once`] and [`EventLoop::run`], terminating the entire
/// event loop. Handlers MUST handle per-connection I/O errors (such as unexpected resets,
/// peer disconnects, or buffer exhaustion) internally—typically by closing and deregistering
/// the affected client socket—and return `Ok(())`. Handlers should return `Err` ONLY for
/// truly unrecoverable, fatal server-wide conditions (e.g. storage corruption).
pub trait Handler {
    /// Invoked when a registered socket file descriptor becomes ready for I/O.
    ///
    /// # Errors
    ///
    /// Returning an [`io::Error`] terminates the entire event loop. Per-connection
    /// I/O failures MUST be handled internally by deregistering the affected client and
    /// returning `Ok(())`.
    fn on_event(
        &mut self,
        event_loop: &mut EventLoop,
        token: Token,
        flags: EpollFlags,
    ) -> io::Result<()>;

    /// Invoked when a scheduled timer deadline expires.
    ///
    /// Default implementation is a no-op returning `Ok(())`.
    ///
    /// # Errors
    ///
    /// May return an [`io::Error`] if timer processing encounters an unrecoverable failure.
    fn on_timeout(&mut self, _event_loop: &mut EventLoop, _timer_id: TimerId) -> io::Result<()> {
        Ok(())
    }

    /// Indicates whether the handler has pending background work requiring periodic ticks.
    ///
    /// When `true`, the event loop caps kernel polling sleep to ~1 ms so [`Handler::on_tick`]
    /// continues running even when no I/O traffic arrives. When `false` (default), the loop sleeps
    /// indefinitely or until the next scheduled timer, consuming 0% idle CPU.
    fn wants_tick(&self) -> bool {
        false
    }

    /// Invoked once per event loop iteration, after expired timers and I/O events have been processed.
    ///
    /// Default implementation is a no-op returning `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returning an [`io::Error`] terminates the entire event loop.
    fn on_tick(&mut self, _event_loop: &mut EventLoop) -> io::Result<()> {
        Ok(())
    }
}

/// An entry in the event loop's timer queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimerEntry {
    deadline: Instant,
    timer_id: TimerId,
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering so BinaryHeap acts as a min-heap (earliest deadline first).
        other.deadline.cmp(&self.deadline)
    }
}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Single-threaded event loop coordinating socket polling and timers.
#[derive(Debug)]
pub struct EventLoop {
    epoll: Epoll,
    timers: BinaryHeap<TimerEntry>,
    events_buffer: Vec<Event>,
    running: bool,
    shutdown_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl EventLoop {
    /// Capacity of the pre-allocated event buffer for receiving batch readiness notifications.
    const DEFAULT_EVENT_CAPACITY: usize = 64;

    /// Maximum duration (in milliseconds) the event loop will block in `epoll_wait`.
    ///
    /// Capping the poll timeout to 100 ms prevents indefinite blocking when a signal
    /// arrives in the race window between the shutdown check and entering kernel sleep.
    const MAX_POLL_TIMEOUT_MS: i32 = 100;

    /// Creates a new `EventLoop` instance backed by the operating system's polling subsystem.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the underlying polling facility cannot be initialized
    /// (e.g. returns [`io::ErrorKind::Unsupported`] on non-Linux platforms).
    pub fn new() -> io::Result<Self> {
        let epoll = Epoll::create()?;
        let events_buffer = vec![Event::new(EpollFlags::empty(), 0); Self::DEFAULT_EVENT_CAPACITY];

        Ok(Self {
            epoll,
            timers: BinaryHeap::new(),
            events_buffer,
            running: false,
            shutdown_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// Returns a thread-safe shared handle to request graceful shutdown of this event loop instance.
    #[must_use]
    pub fn shutdown_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        std::sync::Arc::clone(&self.shutdown_flag)
    }

    /// Requests graceful shutdown of this specific event loop instance.
    pub fn request_shutdown(&self) {
        self.shutdown_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Returns `true` if graceful shutdown was requested on this loop instance or via signal flag.
    #[must_use]
    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown_flag
            .load(std::sync::atomic::Ordering::Relaxed)
            || is_shutdown_requested()
    }

    /// Computes the millisecond timeout for `epoll_wait`, rounding sub-millisecond durations up.
    ///
    /// # Behavior
    ///
    /// - `None`: returns `-1` (infinite wait requested, will be clamped by `MAX_POLL_TIMEOUT_MS`).
    /// - `Some(0)`: returns `0` (immediate non-blocking poll).
    /// - `Some(dur)` with `dur > 0`: ceiling-divided to milliseconds, returning at least `1`.
    #[must_use]
    pub fn compute_timeout_ms(timeout: Option<Duration>) -> i32 {
        match timeout {
            Some(dur) => {
                if dur.is_zero() {
                    0
                } else {
                    let micros = dur.as_micros();
                    let ms_ceil = (micros.saturating_add(999) / 1000).max(1);
                    if ms_ceil > i32::MAX as u128 {
                        i32::MAX
                    } else {
                        ms_ceil as i32
                    }
                }
            }
            None => -1,
        }
    }

    /// Registers a socket file descriptor with the event loop.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if descriptor registration fails (e.g. invalid descriptor or duplicate).
    pub fn register(&self, fd: RawFd, token: Token, flags: EpollFlags) -> io::Result<()> {
        self.epoll.add(fd, flags, token)
    }

    /// Modifies the interest flags or token for an existing registered file descriptor.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if modification fails (e.g. descriptor was not previously registered).
    pub fn reregister(&self, fd: RawFd, token: Token, flags: EpollFlags) -> io::Result<()> {
        self.epoll.modify(fd, flags, token)
    }

    /// Deregisters a socket file descriptor from the event loop.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if deregistration fails.
    pub fn deregister(&self, fd: RawFd) -> io::Result<()> {
        self.epoll.delete(fd)
    }

    /// Schedules a one-shot timer to expire after the specified duration.
    pub fn schedule_timer(&mut self, duration: Duration, timer_id: TimerId) {
        let deadline = Instant::now() + duration;
        self.timers.push(TimerEntry { deadline, timer_id });
    }

    /// Executes a single polling tick of the event loop.
    ///
    /// This method:
    /// 1. Computes the next sleep duration based on caller `timeout` and earliest timer deadline.
    /// 2. Polls the kernel for ready I/O events.
    /// 3. Dispatches any timers that have expired.
    /// 4. Dispatches all ready I/O events to the provided [`Handler`].
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if OS polling or handler processing fails.
    pub fn run_once<H: Handler>(
        &mut self,
        handler: &mut H,
        timeout: Option<Duration>,
    ) -> io::Result<usize> {
        self.run_once_internal(handler, timeout, false)
    }

    /// Executes a single polling tick while bypassing the shutdown request check.
    ///
    /// This allows servers to drain in-flight output buffers and process close events
    /// during graceful shutdown even after a termination signal has been flagged.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if OS polling or handler processing fails.
    pub fn drain_once<H: Handler>(
        &mut self,
        handler: &mut H,
        timeout: Option<Duration>,
    ) -> io::Result<usize> {
        self.run_once_internal(handler, timeout, true)
    }

    fn run_once_internal<H: Handler>(
        &mut self,
        handler: &mut H,
        timeout: Option<Duration>,
        allow_drain: bool,
    ) -> io::Result<usize> {
        if !allow_drain && self.is_shutdown_requested() {
            self.stop();
            return Ok(0);
        }

        let now = Instant::now();

        // Calculate the maximum duration we can sleep without missing the earliest timer.
        let timer_timeout = self
            .timers
            .peek()
            .map(|earliest| earliest.deadline.saturating_duration_since(now));

        let mut effective_timeout = match (timeout, timer_timeout) {
            (Some(user_dur), Some(timer_dur)) => Some(user_dur.min(timer_dur)),
            (Some(user_dur), None) => Some(user_dur),
            (None, Some(timer_dur)) => Some(timer_dur),
            (None, None) => None,
        };

        if handler.wants_tick() {
            const TICK_POLL_TIMEOUT: Duration = Duration::from_millis(1);
            effective_timeout = Some(match effective_timeout {
                Some(dur) => dur.min(TICK_POLL_TIMEOUT),
                None => TICK_POLL_TIMEOUT,
            });
        }

        // Round up sub-millisecond durations and clamp to MAX_POLL_TIMEOUT_MS:
        let raw_timeout_ms = Self::compute_timeout_ms(effective_timeout);
        let timeout_ms = if (0..=Self::MAX_POLL_TIMEOUT_MS).contains(&raw_timeout_ms) {
            raw_timeout_ms
        } else {
            Self::MAX_POLL_TIMEOUT_MS
        };

        // 1. Poll kernel for I/O readiness events with zero per-tick allocation.
        let num_events = self.epoll.wait(&mut self.events_buffer, timeout_ms)?;

        if !allow_drain && self.is_shutdown_requested() {
            self.stop();
            return Ok(0);
        }

        // 2. Dispatch all expired timers.
        let now_after_wait = Instant::now();
        let mut expired_timers = Vec::new();
        while let Some(earliest) = self.timers.peek() {
            if earliest.deadline <= now_after_wait {
                if let Some(entry) = self.timers.pop() {
                    expired_timers.push(entry.timer_id);
                }
            } else {
                break;
            }
        }
        for timer_id in expired_timers {
            handler.on_timeout(self, timer_id)?;
        }

        // 3. Dispatch I/O events.
        let mut ready_events = [Event::new(EpollFlags::empty(), 0); Self::DEFAULT_EVENT_CAPACITY];
        let copy_len = num_events.min(Self::DEFAULT_EVENT_CAPACITY);
        ready_events[..copy_len].copy_from_slice(&self.events_buffer[..copy_len]);
        for &event in &ready_events[..copy_len] {
            handler.on_event(self, event.token(), event.flags())?;
        }

        // 4. Dispatch background tick.
        handler.on_tick(self)?;

        Ok(num_events)
    }

    /// Runs the event loop continuously until [`EventLoop::stop`] is called.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if any polling tick or handler callback returns an error.
    pub fn run<H: Handler>(&mut self, handler: &mut H) -> io::Result<()> {
        self.running = true;
        while self.running {
            if self.is_shutdown_requested() {
                self.stop();
                break;
            }
            self.run_once(handler, None)?;
        }
        Ok(())
    }

    /// Stops the running event loop gracefully after the current tick completes.
    pub fn stop(&mut self) {
        self.running = false;
    }

    /// Returns `true` if the event loop is currently in running state.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    struct MockHandler {
        events: Vec<(Token, EpollFlags)>,
        timeouts: Vec<TimerId>,
    }

    #[cfg(target_os = "linux")]
    impl MockHandler {
        fn new() -> Self {
            Self {
                events: Vec::new(),
                timeouts: Vec::new(),
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl Handler for MockHandler {
        fn on_event(
            &mut self,
            _event_loop: &mut EventLoop,
            token: Token,
            flags: EpollFlags,
        ) -> io::Result<()> {
            self.events.push((token, flags));
            Ok(())
        }

        fn on_timeout(&mut self, _event_loop: &mut EventLoop, timer_id: TimerId) -> io::Result<()> {
            self.timeouts.push(timer_id);
            Ok(())
        }
    }

    #[test]
    fn test_compute_timeout_ms_rounding_and_limits() {
        // Zero duration becomes 0 ms (immediate poll)
        assert_eq!(EventLoop::compute_timeout_ms(Some(Duration::ZERO)), 0);

        // Sub-millisecond (500 micros) must round up to 1 ms, NOT truncate to 0!
        assert_eq!(
            EventLoop::compute_timeout_ms(Some(Duration::from_micros(500))),
            1
        );
        assert_eq!(
            EventLoop::compute_timeout_ms(Some(Duration::from_nanos(1))),
            1
        );

        // Exact millisecond values
        assert_eq!(
            EventLoop::compute_timeout_ms(Some(Duration::from_millis(5))),
            5
        );
        assert_eq!(
            EventLoop::compute_timeout_ms(Some(Duration::from_micros(5001))),
            6
        );

        // None duration becomes -1 (infinite poll request)
        assert_eq!(EventLoop::compute_timeout_ms(None), -1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_event_loop_creation() -> io::Result<()> {
        let el = EventLoop::new()?;
        assert!(!el.is_running());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_timer_scheduling_and_dispatch() -> io::Result<()> {
        let mut el = EventLoop::new()?;
        let mut handler = MockHandler::new();

        // Schedule timer with 0 ms duration (already expired).
        el.schedule_timer(Duration::from_millis(0), 101);
        el.schedule_timer(Duration::from_millis(0), 102);

        let _ = el.run_once(&mut handler, Some(Duration::from_millis(1)))?;

        assert_eq!(handler.timeouts.len(), 2);
        assert!(handler.timeouts.contains(&101));
        assert!(handler.timeouts.contains(&102));

        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_event_loop_stop() -> io::Result<()> {
        let mut el = EventLoop::new()?;
        let mut handler = MockHandler::new();

        el.running = true;
        assert!(el.is_running());

        el.stop();
        assert!(!el.is_running());

        // Calling run_once when stopped should still work cleanly.
        let count = el.run_once(&mut handler, Some(Duration::from_millis(0)))?;
        assert_eq!(count, 0);

        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_event_loop_instance_shutdown_flag() -> io::Result<()> {
        let mut el = EventLoop::new()?;
        let mut handler = MockHandler::new();

        // Request shutdown on loop instance without touching global signal flag:
        el.request_shutdown();
        assert!(el.is_shutdown_requested());
        assert!(!crate::net::sys::is_shutdown_requested());

        el.run(&mut handler)?;
        assert!(!el.is_running());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_event_loop_graceful_shutdown_flag() -> io::Result<()> {
        let mut el = EventLoop::new()?;
        let mut handler = MockHandler::new();

        crate::net::sys::request_shutdown();
        el.run(&mut handler)?;
        assert!(!el.is_running());

        // Reset the flag for other tests.
        crate::net::sys::reset_shutdown();
        Ok(())
    }

    #[cfg(target_os = "linux")]
    struct TickHandler {
        ticks: usize,
        wants: bool,
    }

    #[cfg(target_os = "linux")]
    impl Handler for TickHandler {
        fn on_event(
            &mut self,
            _event_loop: &mut EventLoop,
            _token: Token,
            _flags: EpollFlags,
        ) -> io::Result<()> {
            Ok(())
        }

        fn wants_tick(&self) -> bool {
            self.wants
        }

        fn on_tick(&mut self, _event_loop: &mut EventLoop) -> io::Result<()> {
            self.ticks += 1;
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_event_loop_tick_hook() -> io::Result<()> {
        let mut el = EventLoop::new()?;
        let mut handler = TickHandler {
            ticks: 0,
            wants: true,
        };

        // Even with no traffic, run_once with wants_tick = true executes on_tick
        let _ = el.run_once(&mut handler, Some(Duration::from_millis(5)))?;
        assert_eq!(handler.ticks, 1);

        let _ = el.run_once(&mut handler, Some(Duration::from_millis(5)))?;
        assert_eq!(handler.ticks, 2);

        // With wants_tick = false, on_tick is still called after the poll
        handler.wants = false;
        let _ = el.run_once(&mut handler, Some(Duration::from_millis(0)))?;
        assert_eq!(handler.ticks, 3);

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_event_loop_unsupported_on_non_linux() {
        let res = EventLoop::new();
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
