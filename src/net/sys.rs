//! Low-level operating system I/O primitives and system call abstractions.
//!
//! This module encapsulates low-level operating system calls required for event-driven
//! networking. On Linux, this wraps the `epoll` kernel subsystem using `libc`.
//!
//! # Safety Invariants
//!
//! To keep the rest of the Cachedb codebase 100% safe Rust, all FFI declarations and
//! `unsafe` blocks are strictly quarantined within this module. Every `unsafe` block
//! contains an explicit `// SAFETY:` comment justifying why it is sound.

#![allow(unsafe_code)]

use std::io;
use std::ops::{BitOr, BitOrAssign};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
/// Raw OS file descriptor on Unix systems.
pub type RawFd = std::os::fd::RawFd;

#[cfg(not(unix))]
/// Portable integer representation of an OS file descriptor on non-Unix systems.
pub type RawFd = i32;

#[cfg(unix)]
/// Extracts the raw file descriptor from a standard I/O type on Unix systems.
#[must_use]
pub fn as_raw_fd<T: std::os::fd::AsRawFd>(item: &T) -> RawFd {
    item.as_raw_fd()
}

#[cfg(not(unix))]
/// Extracts the raw socket descriptor as a portable integer on non-Unix systems.
#[must_use]
pub fn as_raw_fd<T: std::os::windows::io::AsRawSocket>(item: &T) -> RawFd {
    item.as_raw_socket() as RawFd
}

/// Interest flags for socket event notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EpollFlags {
    bits: u32,
}

impl EpollFlags {
    /// Socket is ready for non-blocking read operations, or listener has incoming connections.
    pub const READABLE: Self = Self { bits: 1 << 0 };

    /// Socket is ready for non-blocking write operations.
    pub const WRITABLE: Self = Self { bits: 1 << 1 };

    /// Error condition occurred on the associated socket.
    pub const ERROR: Self = Self { bits: 1 << 2 };

    /// Peer closed connection or stream was hung up.
    pub const HANGUP: Self = Self { bits: 1 << 3 };

    /// Edge-triggered event notification mode.
    pub const EDGE_TRIGGERED: Self = Self { bits: 1 << 4 };

    /// Creates an empty set of event flags.
    #[must_use]
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Combines two sets of flags into a union.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    /// Checks whether this flag set contains the specified flag.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.bits & other.bits) == other.bits
    }
}

impl BitOr for EpollFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

impl BitOrAssign for EpollFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.bits |= rhs.bits;
    }
}

/// A ready event returned by the kernel's polling notification mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    flags: EpollFlags,
    token: u64,
}

impl Event {
    /// Creates a new event record with associated flags and user token.
    #[must_use]
    pub const fn new(flags: EpollFlags, token: u64) -> Self {
        Self { flags, token }
    }

    /// Returns the event flags triggered on the descriptor.
    #[must_use]
    pub const fn flags(&self) -> EpollFlags {
        self.flags
    }

    /// Returns the user token associated with the descriptor during registration.
    #[must_use]
    pub const fn token(&self) -> u64 {
        self.token
    }
}

/// Safe RAII wrapper around a Linux `epoll` kernel file descriptor.
#[derive(Debug)]
pub struct Epoll {
    #[cfg(target_os = "linux")]
    fd: RawFd,
    #[cfg(not(target_os = "linux"))]
    _mock_token: u64,
}

#[cfg(target_os = "linux")]
impl Epoll {
    /// Creates a new `epoll` instance with close-on-exec (`EPOLL_CLOEXEC`) enabled.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the kernel fails to allocate an epoll file descriptor.
    pub fn create() -> io::Result<Self> {
        // SAFETY: `epoll_create1` is a kernel system call taking an integer flag.
        // `EPOLL_CLOEXEC` ensures the descriptor is automatically closed on exec.
        let fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd })
    }

    /// Adds a file descriptor to the interest list.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if registration fails (e.g. `EEXIST` if already registered).
    pub fn add(&self, fd: RawFd, flags: EpollFlags, token: u64) -> io::Result<()> {
        self.ctl(libc::EPOLL_CTL_ADD, fd, flags, token)
    }

    /// Modifies the interest list for an already registered file descriptor.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if modification fails (e.g. `ENOENT` if not registered).
    pub fn modify(&self, fd: RawFd, flags: EpollFlags, token: u64) -> io::Result<()> {
        self.ctl(libc::EPOLL_CTL_MOD, fd, flags, token)
    }

    /// Removes a file descriptor from the interest list.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if deregistration fails.
    pub fn delete(&self, fd: RawFd) -> io::Result<()> {
        // SAFETY: In Linux >= 2.6.9, `epoll_ctl` with `EPOLL_CTL_DEL` does not inspect
        // the event pointer. Passing a null pointer is valid and documented practice.
        let res =
            unsafe { libc::epoll_ctl(self.fd, libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut()) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Waits for readiness events on the registered file descriptors.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if `epoll_wait` fails. If interrupted by a signal (`EINTR`),
    /// this function handles it gracefully and returns `Ok(0)`.
    pub fn wait(&self, events: &mut [Event], timeout_ms: i32) -> io::Result<usize> {
        let mut raw_events = vec![libc::epoll_event { events: 0, u64: 0 }; events.len()];

        // SAFETY: `raw_events` is a properly allocated buffer with length `events.len()`.
        // `self.fd` is an open epoll descriptor owned by `self`.
        let count = unsafe {
            libc::epoll_wait(
                self.fd,
                raw_events.as_mut_ptr(),
                raw_events.len() as libc::c_int,
                timeout_ms,
            )
        };

        if count < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            return Err(err);
        }

        let num_ready = count as usize;
        for i in 0..num_ready {
            events[i] = Self::from_raw_event(&raw_events[i]);
        }

        Ok(num_ready)
    }

    fn ctl(&self, op: libc::c_int, fd: RawFd, flags: EpollFlags, token: u64) -> io::Result<()> {
        let mut raw_event = libc::epoll_event {
            events: Self::to_raw_flags(flags),
            u64: token,
        };

        // SAFETY: `&mut raw_event` is a valid pointer to a stack-allocated `libc::epoll_event` struct.
        // `self.fd` is an open epoll descriptor owned by `self`.
        let res = unsafe { libc::epoll_ctl(self.fd, op, fd, &mut raw_event) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn to_raw_flags(flags: EpollFlags) -> u32 {
        let mut raw = 0u32;
        if flags.contains(EpollFlags::READABLE) {
            raw |= libc::EPOLLIN as u32;
        }
        if flags.contains(EpollFlags::WRITABLE) {
            raw |= libc::EPOLLOUT as u32;
        }
        if flags.contains(EpollFlags::ERROR) {
            raw |= libc::EPOLLERR as u32;
        }
        if flags.contains(EpollFlags::HANGUP) {
            raw |= libc::EPOLLHUP as u32;
        }
        if flags.contains(EpollFlags::EDGE_TRIGGERED) {
            raw |= libc::EPOLLET as u32;
        }
        raw
    }

    fn from_raw_event(raw: &libc::epoll_event) -> Event {
        let mut flags = EpollFlags::empty();
        let ev = raw.events;
        if (ev & (libc::EPOLLIN as u32)) != 0 {
            flags = flags.union(EpollFlags::READABLE);
        }
        if (ev & (libc::EPOLLOUT as u32)) != 0 {
            flags = flags.union(EpollFlags::WRITABLE);
        }
        if (ev & (libc::EPOLLERR as u32)) != 0 {
            flags = flags.union(EpollFlags::ERROR);
        }
        if (ev & (libc::EPOLLHUP as u32)) != 0 {
            flags = flags.union(EpollFlags::HANGUP);
        }
        Event::new(flags, raw.u64)
    }
}

#[cfg(target_os = "linux")]
impl Drop for Epoll {
    fn drop(&mut self) {
        // SAFETY: `self.fd` is an owned open file descriptor that must be closed when dropped.
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Epoll {
    /// Creates a mock epoll instance for non-Linux compilation.
    ///
    /// # Errors
    ///
    /// On non-Linux targets, native kernel epoll is not available.
    pub fn create() -> io::Result<Self> {
        Ok(Self { _mock_token: 0 })
    }

    /// Adds a file descriptor to the interest list (stub on non-Linux).
    ///
    /// # Errors
    ///
    /// Never returns an error in mock mode.
    pub fn add(&self, _fd: RawFd, _flags: EpollFlags, _token: u64) -> io::Result<()> {
        Ok(())
    }

    /// Modifies a file descriptor in the interest list (stub on non-Linux).
    ///
    /// # Errors
    ///
    /// Never returns an error in mock mode.
    pub fn modify(&self, _fd: RawFd, _flags: EpollFlags, _token: u64) -> io::Result<()> {
        Ok(())
    }

    /// Deletes a file descriptor from the interest list (stub on non-Linux).
    ///
    /// # Errors
    ///
    /// Never returns an error in mock mode.
    pub fn delete(&self, _fd: RawFd) -> io::Result<()> {
        Ok(())
    }

    /// Waits for readiness events (stub on non-Linux, returns 0 events).
    ///
    /// # Errors
    ///
    /// Never returns an error in mock mode.
    pub fn wait(&self, _events: &mut [Event], _timeout_ms: i32) -> io::Result<usize> {
        Ok(0)
    }
}

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Returns `true` if a termination signal (such as `SIGINT` or `SIGTERM`) was received
/// or if a shutdown was programmatically requested.
#[must_use]
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

/// Sets the shutdown flag, signaling the event loop to terminate gracefully.
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

/// Resets the shutdown flag back to `false`.
pub fn reset_shutdown() {
    SHUTDOWN_REQUESTED.store(false, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
extern "C" fn sig_handler(_sig: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
/// Registers signal handlers for `SIGINT` and `SIGTERM` to initiate graceful shutdown.
///
/// # Errors
///
/// Returns an [`io::Error`] if the underlying `sigaction` system call fails.
pub fn install_shutdown_signal_handlers() -> io::Result<()> {
    // SAFETY:
    // - `sa` is zero-initialized, which creates a valid empty `libc::sigaction` structure.
    // - `sa.sa_sigaction` is cast from `sig_handler`, an extern "C" function with a signature
    //   matching C signal handlers (`fn(c_int)`).
    // - `sigemptyset` is called with a valid pointer to `sa.sa_mask`.
    // - `sa_flags` is set to 0 (specifically omitting `SA_RESTART`) so that blocking calls
    //   such as `epoll_wait` are interrupted with `EINTR`, waking up the event loop immediately.
    // - `libc::sigaction` is called with valid POSIX signal numbers `SIGINT` and `SIGTERM`.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = sig_handler as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0;

        if libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut()) < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut()) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
/// Registers signal handlers for `SIGINT` and `SIGTERM` (stub for non-Linux targets).
///
/// # Errors
///
/// Never returns an error on non-Linux platforms.
pub fn install_shutdown_signal_handlers() -> io::Result<()> {
    Ok(())
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

    #[test]
    fn test_epoll_flags_operations() {
        let read = EpollFlags::READABLE;
        let write = EpollFlags::WRITABLE;
        let combined = read | write;

        assert!(combined.contains(EpollFlags::READABLE));
        assert!(combined.contains(EpollFlags::WRITABLE));
        assert!(!combined.contains(EpollFlags::ERROR));
    }

    #[test]
    fn test_event_creation() {
        let flags = EpollFlags::READABLE | EpollFlags::HANGUP;
        let event = Event::new(flags, 42);

        assert_eq!(event.token(), 42);
        assert!(event.flags().contains(EpollFlags::READABLE));
        assert!(event.flags().contains(EpollFlags::HANGUP));
        assert!(!event.flags().contains(EpollFlags::WRITABLE));
    }

    #[test]
    fn test_shutdown_flag_operations() {
        reset_shutdown();
        assert!(!is_shutdown_requested());

        request_shutdown();
        assert!(is_shutdown_requested());

        reset_shutdown();
        assert!(!is_shutdown_requested());

        // Calling install_shutdown_signal_handlers should succeed.
        let result = install_shutdown_signal_handlers();
        assert!(result.is_ok());
    }
}
