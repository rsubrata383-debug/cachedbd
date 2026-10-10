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
pub trait Handler {
    /// Invoked when a registered socket file descriptor becomes ready for I/O.
    ///
    /// # Errors
    ///
    /// May return an [`io::Error`] if processing the event encounters an unrecoverable I/O failure.
    fn on_event(&mut self, token: Token, flags: EpollFlags) -> io::Result<()>;

    /// Invoked when a scheduled timer deadline expires.
    ///
    /// Default implementation is a no-op returning `Ok(())`.
    ///
    /// # Errors
    ///
    /// May return an [`io::Error`] if timer processing encounters an unrecoverable failure.
    fn on_timeout(&mut self, _timer_id: TimerId) -> io::Result<()> {
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
}

impl EventLoop {
    /// Capacity of the pre-allocated event buffer for receiving batch readiness notifications.
    const DEFAULT_EVENT_CAPACITY: usize = 64;

    /// Creates a new `EventLoop` instance backed by the operating system's polling subsystem.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the underlying polling facility cannot be initialized.
    pub fn new() -> io::Result<Self> {
        let epoll = Epoll::create()?;
        let events_buffer = vec![Event::new(EpollFlags::empty(), 0); Self::DEFAULT_EVENT_CAPACITY];

        Ok(Self {
            epoll,
            timers: BinaryHeap::new(),
            events_buffer,
            running: false,
        })
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
        if !allow_drain && is_shutdown_requested() {
            self.stop();
            return Ok(0);
        }

        let now = Instant::now();

        // Calculate the maximum duration we can sleep without missing the earliest timer.
        let timer_timeout = self
            .timers
            .peek()
            .map(|earliest| earliest.deadline.saturating_duration_since(now));

        let effective_timeout = match (timeout, timer_timeout) {
            (Some(user_dur), Some(timer_dur)) => Some(user_dur.min(timer_dur)),
            (Some(user_dur), None) => Some(user_dur),
            (None, Some(timer_dur)) => Some(timer_dur),
            (None, None) => None,
        };

        // Convert duration to milliseconds for epoll_wait (-1 = infinite block).
        let timeout_ms = match effective_timeout {
            Some(dur) => {
                let ms = dur.as_millis();
                if ms > i32::MAX as u128 {
                    i32::MAX
                } else {
                    ms as i32
                }
            }
            None => -1,
        };

        // 1. Poll kernel for I/O readiness events.
        let num_events = match self.epoll.wait(&mut self.events_buffer, timeout_ms) {
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Unsupported => {
                // Non-Linux platforms or mock environment where real epoll is unavailable.
                0
            }
            Err(err) => return Err(err),
        };

        if !allow_drain && is_shutdown_requested() {
            self.stop();
            return Ok(0);
        }

        // 2. Dispatch all expired timers.
        let now_after_wait = Instant::now();
        while let Some(earliest) = self.timers.peek() {
            if earliest.deadline <= now_after_wait {
                if let Some(entry) = self.timers.pop() {
                    handler.on_timeout(entry.timer_id)?;
                }
            } else {
                break;
            }
        }

        // 3. Dispatch I/O events.
        for i in 0..num_events {
            let event = self.events_buffer[i];
            handler.on_event(event.token(), event.flags())?;
        }

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
            if is_shutdown_requested() {
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

    struct MockHandler {
        events: Vec<(Token, EpollFlags)>,
        timeouts: Vec<TimerId>,
    }

    impl MockHandler {
        fn new() -> Self {
            Self {
                events: Vec::new(),
                timeouts: Vec::new(),
            }
        }
    }

    impl Handler for MockHandler {
        fn on_event(&mut self, token: Token, flags: EpollFlags) -> io::Result<()> {
            self.events.push((token, flags));
            Ok(())
        }

        fn on_timeout(&mut self, timer_id: TimerId) -> io::Result<()> {
            self.timeouts.push(timer_id);
            Ok(())
        }
    }

    #[test]
    fn test_event_loop_creation() -> io::Result<()> {
        let el = EventLoop::new()?;
        assert!(!el.is_running());
        Ok(())
    }

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
}
