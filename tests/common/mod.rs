//! Shared test helpers and harness.

/// Deterministic 64-bit XorShift pseudorandom number generator strictly
/// conforming to AGENTS.md rule 11.1.
#[derive(Debug, Clone)]
pub struct XorShift64 {
    state: u64,
}

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
