//! In-memory read buffers and write queues for connection I/O.
//!
//! This module provides:
//! - [`ReadBuffer`]: A contiguous byte buffer that grows dynamically up to a hard ceiling,
//!   compacts unconsumed bytes, and receives data directly from non-blocking sockets.
//! - [`WriteQueue`]: A FIFO queue of byte chunks that flushes outgoing frames to sockets,
//!   handles partial writes without copying, and tracks backpressure watermarks.

use std::collections::VecDeque;
use std::io::{self, Read, Write};

/// Default initial allocation for read buffers (4 KB).
pub const DEFAULT_INITIAL_READ_CAPACITY: usize = 4096;

/// Default maximum memory ceiling for a single connection read buffer (64 MB).
pub const DEFAULT_MAX_READ_CAPACITY: usize = 64 * 1024 * 1024;

/// Default high-watermark threshold for write queues to signal backpressure (10 MB).
pub const DEFAULT_WRITE_HIGH_WATERMARK: usize = 10 * 1024 * 1024;

/// Default low-watermark threshold for write queues to clear backpressure (5 MB).
pub const DEFAULT_WRITE_LOW_WATERMARK: usize = 5 * 1024 * 1024;

/// Default maximum memory ceiling for a single connection write queue (64 MB).
pub const DEFAULT_MAX_WRITE_CAPACITY: usize = 64 * 1024 * 1024;

/// Contiguous growable read buffer for receiving streaming network bytes.
///
/// Data is read into the buffer from non-blocking sockets. Protocol parsers can
/// inspect the unconsumed portion as a contiguous `&[u8]` slice without allocations.
/// Once a message frame is extracted, the caller invokes [`ReadBuffer::consume`]
/// to advance the read cursor.
#[derive(Debug, Clone)]
pub struct ReadBuffer {
    buf: Vec<u8>,
    read_pos: usize,
    write_pos: usize,
    max_capacity: usize,
}

impl Default for ReadBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadBuffer {
    /// Creates a new read buffer with default initial capacity (4 KB) and default max capacity (64 MB).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacities(DEFAULT_INITIAL_READ_CAPACITY, DEFAULT_MAX_READ_CAPACITY)
    }

    /// Creates a new read buffer with explicit initial capacity and maximum capacity ceiling.
    #[must_use]
    pub fn with_capacities(initial_capacity: usize, max_capacity: usize) -> Self {
        let actual_initial = initial_capacity.min(max_capacity).max(1);
        Self {
            buf: vec![0u8; actual_initial],
            read_pos: 0,
            write_pos: 0,
            max_capacity,
        }
    }

    /// Returns a slice of the received, unconsumed bytes currently stored in the buffer.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.buf[self.read_pos..self.write_pos]
    }

    /// Returns the number of unconsumed bytes currently available in the buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.write_pos - self.read_pos
    }

    /// Returns `true` if there are no unconsumed bytes in the buffer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read_pos == self.write_pos
    }

    /// Returns the current total capacity of the underlying buffer vector.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Returns the maximum allowed memory ceiling for this buffer.
    #[must_use]
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// Advances the read cursor by `count` bytes, marking that data as processed.
    ///
    /// If all data has been consumed, the internal read and write cursors automatically
    /// reset back to index 0 to maximize subsequent write capacity without copying.
    pub fn consume(&mut self, count: usize) {
        let available = self.len();
        let to_consume = count.min(available);
        self.read_pos += to_consume;

        if self.read_pos == self.write_pos {
            self.read_pos = 0;
            self.write_pos = 0;
        }
    }

    /// Compacts unconsumed data by shifting it to the beginning of the buffer.
    pub fn compact(&mut self) {
        if self.read_pos > 0 {
            let active_len = self.len();
            self.buf.copy_within(self.read_pos..self.write_pos, 0);
            self.read_pos = 0;
            self.write_pos = active_len;
        }
    }

    /// Discards all buffered data and resets the read and write positions to zero.
    pub fn clear(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
    }

    /// Ensures that at least `additional` bytes of contiguous free space are available
    /// at the end of the buffer for subsequent writing.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] with [`io::ErrorKind::OutOfMemory`] if expanding the buffer
    /// would exceed [`ReadBuffer::max_capacity`].
    pub fn reserve(&mut self, additional: usize) -> io::Result<()> {
        if additional == 0 {
            return Ok(());
        }

        let spare_space = self.buf.len() - self.write_pos;
        if spare_space >= additional {
            return Ok(());
        }

        // Attempt compaction first if leading space is available.
        if self.read_pos > 0 {
            self.compact();
            if self.buf.len() - self.write_pos >= additional {
                return Ok(());
            }
        }

        let current_len = self.len();
        let required = current_len
            .checked_add(additional)
            .ok_or_else(|| io::Error::new(io::ErrorKind::OutOfMemory, "buffer size overflow"))?;

        if required > self.max_capacity {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "read buffer capacity exceeded maximum allowed limit",
            ));
        }

        // Double current capacity or jump to required capacity, bounded by max_capacity.
        let mut new_capacity = self.buf.len().saturating_mul(2).max(required);
        if new_capacity > self.max_capacity {
            new_capacity = self.max_capacity;
        }
        if new_capacity < required {
            new_capacity = required;
        }

        self.buf.resize(new_capacity, 0);
        Ok(())
    }

    /// Reads incoming bytes directly from the given reader into the buffer.
    ///
    /// Automatically ensures sufficient write space before invoking `reader.read`.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the underlying read fails or if buffer expansion exceeds limits.
    pub fn read_from<R: Read>(&mut self, reader: &mut R) -> io::Result<usize> {
        let free_space = self.buf.len() - self.write_pos;

        // If free space is tight, attempt to reserve additional room up to max_capacity.
        if free_space < 512 && self.len() < self.max_capacity {
            let remaining_allowed = self.max_capacity.saturating_sub(self.len());
            if remaining_allowed > 0 {
                let target_reserve = 4096.min(remaining_allowed);
                let _ = self.reserve(target_reserve);
            }
        }

        // If the buffer is completely full and cannot expand, report out of memory.
        if self.write_pos >= self.buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "read buffer capacity exceeded maximum allowed limit",
            ));
        }

        let n = reader.read(&mut self.buf[self.write_pos..])?;
        self.write_pos += n;
        Ok(n)
    }
}

/// FIFO write queue holding pending outgoing data chunks for socket transmission.
///
/// Designed for zero-copy enqueue of serialized frames, tracking partial write progress,
/// and signaling backpressure using two-watermark hysteresis ($W_{\text{high}}$ and $W_{\text{low}}$).
#[derive(Debug, Clone)]
pub struct WriteQueue {
    chunks: VecDeque<Vec<u8>>,
    front_offset: usize,
    total_bytes: usize,
    high_watermark: usize,
    low_watermark: usize,
    max_capacity: usize,
    backpressured: bool,
}

impl Default for WriteQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteQueue {
    /// Creates an empty write queue with default limits (10 MB high watermark, 5 MB low watermark, 64 MB max).
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_WRITE_HIGH_WATERMARK,
            DEFAULT_WRITE_LOW_WATERMARK,
            DEFAULT_MAX_WRITE_CAPACITY,
        )
    }

    /// Creates an empty write queue with a specified high-watermark threshold.
    ///
    /// The low watermark is automatically configured to 50% of `high_watermark`.
    #[must_use]
    pub fn with_high_watermark(high_watermark: usize) -> Self {
        Self::with_limits(
            high_watermark,
            high_watermark / 2,
            DEFAULT_MAX_WRITE_CAPACITY,
        )
    }

    /// Creates an empty write queue with explicit high and low watermarks and maximum capacity.
    #[must_use]
    pub fn with_limits(high_watermark: usize, low_watermark: usize, max_capacity: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            front_offset: 0,
            total_bytes: 0,
            high_watermark,
            low_watermark: low_watermark.min(high_watermark),
            max_capacity,
            backpressured: false,
        }
    }

    /// Returns the total number of unwritten pending bytes in the queue.
    #[must_use]
    pub fn len(&self) -> usize {
        self.total_bytes
    }

    /// Returns `true` if there are no pending bytes queued for transmission.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_bytes == 0
    }

    /// Returns `true` if backpressure is currently active.
    ///
    /// Backpressure engages when queued bytes reach [`WriteQueue::high_watermark`],
    /// and disengages only after pending bytes drain below [`WriteQueue::low_watermark`].
    #[must_use]
    pub fn is_backpressured(&self) -> bool {
        self.backpressured
    }

    /// Returns the configured high-watermark threshold in bytes.
    #[must_use]
    pub fn high_watermark(&self) -> usize {
        self.high_watermark
    }

    /// Returns the configured low-watermark threshold in bytes.
    #[must_use]
    pub fn low_watermark(&self) -> usize {
        self.low_watermark
    }

    /// Returns the configured maximum capacity ceiling in bytes.
    #[must_use]
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// Enqueues a borrowed byte slice by copying it into a new queue chunk.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] with [`io::ErrorKind::OutOfMemory`] if the queued bytes
    /// would exceed [`WriteQueue::max_capacity`].
    pub fn enqueue(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }

        let new_total = self.total_bytes.checked_add(data.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::OutOfMemory, "write queue size overflow")
        })?;

        if new_total > self.max_capacity {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "write queue exceeded maximum allowed capacity limit",
            ));
        }

        self.total_bytes = new_total;
        self.chunks.push_back(data.to_vec());

        if self.total_bytes >= self.high_watermark {
            self.backpressured = true;
        }

        Ok(())
    }

    /// Enqueues an owned byte vector with zero allocation or data copying.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] with [`io::ErrorKind::OutOfMemory`] if the queued bytes
    /// would exceed [`WriteQueue::max_capacity`].
    pub fn enqueue_chunk(&mut self, chunk: Vec<u8>) -> io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }

        let new_total = self.total_bytes.checked_add(chunk.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::OutOfMemory, "write queue size overflow")
        })?;

        if new_total > self.max_capacity {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "write queue exceeded maximum allowed capacity limit",
            ));
        }

        self.total_bytes = new_total;
        self.chunks.push_back(chunk);

        if self.total_bytes >= self.high_watermark {
            self.backpressured = true;
        }

        Ok(())
    }

    /// Flushes queued bytes to the provided writer until the queue is drained or `WouldBlock` occurs.
    ///
    /// Disengages backpressure once pending bytes fall below [`WriteQueue::low_watermark`].
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if writing to the output stream encounters an error.
    pub fn write_to<W: Write>(&mut self, writer: &mut W) -> io::Result<usize> {
        let mut total_written = 0;

        while let Some(front) = self.chunks.front() {
            let pending_slice = &front[self.front_offset..];
            match writer.write(pending_slice) {
                Ok(0) => break,
                Ok(n) => {
                    total_written += n;
                    self.total_bytes = self.total_bytes.saturating_sub(n);
                    self.front_offset += n;

                    if self.front_offset >= front.len() {
                        self.chunks.pop_front();
                        self.front_offset = 0;
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if self.total_bytes <= self.low_watermark {
                        self.backpressured = false;
                    }
                    if total_written > 0 {
                        return Ok(total_written);
                    }
                    return Err(err);
                }
                Err(err) => {
                    if self.total_bytes <= self.low_watermark {
                        self.backpressured = false;
                    }
                    return Err(err);
                }
            }
        }

        if self.total_bytes <= self.low_watermark {
            self.backpressured = false;
        }

        Ok(total_written)
    }

    /// Discards all queued chunks and resets total byte counts to zero.
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.front_offset = 0;
        self.total_bytes = 0;
        self.backpressured = false;
    }
}

/// Circular ring buffer with wrap-around indexing for zero-copy I/O.
///
/// Unlike a linear buffer that shifts data when unconsumed bytes remain,
/// a circular ring buffer maintains read and write cursors that wrap around
/// the buffer length. Data can be inspected as two slices via [`RingBuffer::as_slices`],
/// linearized on demand via [`RingBuffer::make_contiguous`], or read directly from a stream
/// via [`RingBuffer::read_from`].
#[derive(Debug, Clone)]
pub struct RingBuffer {
    buf: Vec<u8>,
    head: usize,
    tail: usize,
    len: usize,
    max_capacity: usize,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RingBuffer {
    /// Creates a new ring buffer with default initial capacity (4 KB) and default max capacity (64 MB).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacities(DEFAULT_INITIAL_READ_CAPACITY, DEFAULT_MAX_READ_CAPACITY)
    }

    /// Creates a new ring buffer with explicit initial capacity and maximum capacity ceiling.
    #[must_use]
    pub fn with_capacities(initial_capacity: usize, max_capacity: usize) -> Self {
        let actual_initial = initial_capacity.min(max_capacity).max(1);
        Self {
            buf: vec![0u8; actual_initial],
            head: 0,
            tail: 0,
            len: 0,
            max_capacity,
        }
    }

    /// Returns the number of unconsumed bytes currently stored in the ring buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if there are no unconsumed bytes in the ring buffer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the current total capacity of the underlying buffer vector.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Returns the maximum allowed memory ceiling for this ring buffer.
    #[must_use]
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// Returns the remaining free space available for writing before expansion is required.
    #[must_use]
    pub fn available_space(&self) -> usize {
        self.buf.len().saturating_sub(self.len)
    }

    /// Returns `true` if the buffered data wraps around the physical buffer boundary.
    #[must_use]
    pub fn is_wrapped(&self) -> bool {
        self.len > 0 && self.tail + self.len > self.buf.len()
    }

    /// Returns two contiguous slices representing the current buffered data.
    ///
    /// If the data does not wrap around the end of the buffer, the second slice will be empty.
    #[must_use]
    pub fn as_slices(&self) -> (&[u8], &[u8]) {
        if self.len == 0 {
            (&[], &[])
        } else if self.tail + self.len <= self.buf.len() {
            (&self.buf[self.tail..self.tail + self.len], &[])
        } else {
            (&self.buf[self.tail..], &self.buf[..self.head])
        }
    }

    /// Linearizes the buffered data if wrapped, returning a single contiguous byte slice.
    pub fn make_contiguous(&mut self) -> &[u8] {
        if self.len == 0 || self.tail + self.len <= self.buf.len() {
            return &self.buf[self.tail..self.tail + self.len];
        }

        let mut linearized = Vec::with_capacity(self.buf.len());
        linearized.extend_from_slice(&self.buf[self.tail..]);
        linearized.extend_from_slice(&self.buf[..self.head]);
        linearized.resize(self.buf.len(), 0);

        self.buf = linearized;
        self.tail = 0;
        self.head = self.len;

        &self.buf[0..self.len]
    }

    /// Advances the read cursor by `count` bytes, discarding consumed data.
    pub fn consume(&mut self, count: usize) {
        let to_consume = count.min(self.len);
        self.tail = (self.tail + to_consume) % self.buf.len();
        self.len -= to_consume;

        if self.len == 0 {
            self.tail = 0;
            self.head = 0;
        }
    }

    /// Ensures that at least `additional` bytes of free space are available for writing.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] with [`io::ErrorKind::OutOfMemory`] if expansion would exceed
    /// [`RingBuffer::max_capacity`].
    pub fn reserve(&mut self, additional: usize) -> io::Result<()> {
        if self.available_space() >= additional {
            return Ok(());
        }

        let required = self.len.checked_add(additional).ok_or_else(|| {
            io::Error::new(io::ErrorKind::OutOfMemory, "ring buffer size overflow")
        })?;

        if required > self.max_capacity {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "ring buffer capacity exceeded maximum allowed limit",
            ));
        }

        let mut new_cap = self.buf.len().saturating_mul(2).max(required);
        if new_cap > self.max_capacity {
            new_cap = self.max_capacity;
        }
        if new_cap < required {
            new_cap = required;
        }

        let (s1, s2) = self.as_slices();
        let mut new_buf = vec![0u8; new_cap];
        new_buf[0..s1.len()].copy_from_slice(s1);
        new_buf[s1.len()..s1.len() + s2.len()].copy_from_slice(s2);

        self.buf = new_buf;
        self.tail = 0;
        self.head = self.len;
        Ok(())
    }

    /// Reads incoming bytes directly from the given reader into the spare ring capacity.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the underlying read fails or if capacity is exhausted.
    pub fn read_from<R: Read>(&mut self, reader: &mut R) -> io::Result<usize> {
        let available = self.available_space();
        if available < 512 && self.len < self.max_capacity {
            let allowed = self.max_capacity.saturating_sub(self.len);
            if allowed > 0 {
                let _ = self.reserve(4096.min(allowed));
            }
        }

        if self.available_space() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "ring buffer capacity exceeded maximum allowed limit",
            ));
        }

        if self.head == self.buf.len() {
            self.head = 0;
        }

        let write_slice = if self.head >= self.tail && self.len > 0 {
            let end_space = self.buf.len() - self.head;
            if end_space > 0 {
                &mut self.buf[self.head..]
            } else {
                self.head = 0;
                &mut self.buf[0..self.tail]
            }
        } else if self.len == 0 {
            self.head = 0;
            self.tail = 0;
            &mut self.buf[..]
        } else {
            &mut self.buf[self.head..self.tail]
        };

        let n = reader.read(write_slice)?;
        self.head = (self.head + n) % self.buf.len();
        self.len += n;
        Ok(n)
    }

    /// Discards all data in the ring buffer, resetting cursors to zero.
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
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

    #[test]
    fn test_read_buffer_basic_read_and_consume() -> io::Result<()> {
        let mut rb = ReadBuffer::with_capacities(128, 1024);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());

        let mut data = io::Cursor::new(b"hello world");
        let n = rb.read_from(&mut data)?;
        assert_eq!(n, 11);
        assert_eq!(rb.len(), 11);
        assert_eq!(rb.data(), b"hello world");

        rb.consume(6);
        assert_eq!(rb.len(), 5);
        assert_eq!(rb.data(), b"world");

        rb.consume(5);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
        Ok(())
    }

    #[test]
    fn test_read_buffer_compact_and_growth() -> io::Result<()> {
        let mut rb = ReadBuffer::with_capacities(64, 128);
        let mut data1 = io::Cursor::new(vec![b'a'; 40]);
        rb.read_from(&mut data1)?;

        // Consume 30 bytes, leaving 10.
        rb.consume(30);
        assert_eq!(rb.len(), 10);

        // Read 50 more bytes, forcing compaction and growth.
        let mut data2 = io::Cursor::new(vec![b'b'; 50]);
        rb.read_from(&mut data2)?;
        assert_eq!(rb.len(), 60);

        // Read until the buffer reaches max capacity (128).
        let mut overflow_data = io::Cursor::new(vec![b'c'; 200]);
        let n = rb.read_from(&mut overflow_data)?;
        assert_eq!(n, 68);
        assert_eq!(rb.len(), 128);

        // Attempting another read when buffer is at max capacity should fail with OutOfMemory.
        let res = rb.read_from(&mut overflow_data);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::OutOfMemory);
        Ok(())
    }

    #[test]
    fn test_write_queue_operations() -> io::Result<()> {
        let mut wq = WriteQueue::with_high_watermark(20);
        assert!(wq.is_empty());
        assert!(!wq.is_backpressured());

        wq.enqueue(b"first chunk; ")?;
        wq.enqueue_chunk(b"second chunk".to_vec())?;
        assert_eq!(wq.len(), 25);
        assert!(wq.is_backpressured());

        let mut output = Vec::new();
        let written = wq.write_to(&mut output)?;
        assert_eq!(written, 25);
        assert_eq!(wq.len(), 0);
        assert!(wq.is_empty());
        assert!(!wq.is_backpressured());
        assert_eq!(&output[..], b"first chunk; second chunk");
        Ok(())
    }

    struct BoundedWriter<W> {
        inner: W,
        remaining_budget: usize,
    }

    impl<W: Write> Write for BoundedWriter<W> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.remaining_budget == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "write limit reached",
                ));
            }
            let to_write = buf.len().min(self.remaining_budget);
            let written = self.inner.write(&buf[..to_write])?;
            self.remaining_budget -= written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    #[test]
    fn test_write_queue_backpressure_hysteresis() -> io::Result<()> {
        // High watermark = 50, Low watermark = 20, Max capacity = 100
        let mut wq = WriteQueue::with_limits(50, 20, 100);
        assert!(!wq.is_backpressured());

        // Enqueue 40 bytes (< 50): not backpressured.
        wq.enqueue(&[1u8; 40])?;
        assert_eq!(wq.len(), 40);
        assert!(!wq.is_backpressured());

        // Enqueue 20 bytes (total 60 >= 50): backpressured engaged!
        wq.enqueue(&[2u8; 20])?;
        assert_eq!(wq.len(), 60);
        assert!(wq.is_backpressured());

        // Exceeding max capacity (100) returns OutOfMemory and leaves queue state intact.
        let err = wq.enqueue(&[3u8; 50]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::OutOfMemory);
        assert_eq!(wq.len(), 60);
        assert!(wq.is_backpressured());

        // Partial flush: write 20 bytes (leaving 40 bytes).
        // Since 40 > low_watermark (20), hysteresis prevents premature unpausing!
        let mut sink1 = BoundedWriter {
            inner: Vec::new(),
            remaining_budget: 20,
        };
        let written1 = wq.write_to(&mut sink1)?;
        assert_eq!(written1, 20);
        assert_eq!(wq.len(), 40);
        assert!(
            wq.is_backpressured(),
            "backpressure should remain active when pending > low_watermark"
        );

        // Partial flush: write 25 bytes (leaving 15 bytes).
        // Since 15 <= low_watermark (20), backpressure is released!
        let mut sink2 = BoundedWriter {
            inner: Vec::new(),
            remaining_budget: 25,
        };
        let written2 = wq.write_to(&mut sink2)?;
        assert_eq!(written2, 25);
        assert_eq!(wq.len(), 15);
        assert!(
            !wq.is_backpressured(),
            "backpressure should clear when pending <= low_watermark"
        );

        // Draining remaining 15 bytes.
        let mut sink3 = Vec::new();
        let written3 = wq.write_to(&mut sink3)?;
        assert_eq!(written3, 15);
        assert_eq!(wq.len(), 0);
        assert!(!wq.is_backpressured());

        Ok(())
    }

    #[test]
    fn test_ring_buffer_operations() -> io::Result<()> {
        let mut ring = RingBuffer::with_capacities(16, 16);
        assert_eq!(ring.len(), 0);
        assert!(ring.is_empty());

        let mut data1 = io::Cursor::new(b"0123456789".to_vec());
        let n1 = ring.read_from(&mut data1)?;
        assert_eq!(n1, 10);
        assert_eq!(ring.len(), 10);
        assert!(!ring.is_wrapped());

        let (s1, s2) = ring.as_slices();
        assert_eq!(s1, b"0123456789");
        assert_eq!(s2, b"");

        ring.consume(6);
        assert_eq!(ring.len(), 4);
        let (s1, _) = ring.as_slices();
        assert_eq!(s1, b"6789");

        // Read 10 more bytes, which will wrap around the 16-byte ring.
        let mut data2 = io::Cursor::new(b"abcdefghij".to_vec());
        let n2 = ring.read_from(&mut data2)?;
        assert_eq!(n2, 6); // First fills end of ring (16 - 10 = 6)
        assert_eq!(ring.len(), 10);

        let n3 = ring.read_from(&mut data2)?;
        assert_eq!(n3, 4); // Wraps around to index 0
        assert_eq!(ring.len(), 14);
        assert!(ring.is_wrapped());

        let (s1, s2) = ring.as_slices();
        assert_eq!(s1, b"6789abcdef");
        assert_eq!(s2, b"ghij");

        // make_contiguous should linearize the two slices into one.
        let contiguous = ring.make_contiguous();
        assert_eq!(contiguous, b"6789abcdefghij");
        assert!(!ring.is_wrapped());

        Ok(())
    }
}
