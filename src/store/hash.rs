//! Fast 64-bit non-cryptographic hash function with per-process random seeding.
//!
//! # Algorithm Design
//!
//! This implementation is based on the **Wyhash** (v4) design by Wang Yi, widely recognized
//! for its high throughput, minimal instruction count, low collision rates, and excellent
//! avalanche characteristics.
//!
//! The core mixing primitive is `mum` (multiply and fold):
//!
//! ```text
//! mum(a, b) = (lo ^ hi) where (lo, hi) are the 64-bit halves of (a * b as u128)
//! ```
//!
//! Input bytes are consumed in 8-byte little-endian words using [`chunks_exact`](slice::chunks_exact).
//! Remaining tail bytes (0 to 7) and total input length are folded into two parallel accumulators
//! using non-linear mixing passes.
//!
//! # HashDoS and Cryptographic Warning
//!
//! **This hash function is strictly non-cryptographic.** It is designed for in-memory hash tables,
//! not for digital signatures or password hashing.
//!
//! To mitigate HashDoS attacks (where an external attacker submits carefully crafted keys to force
//! hash collisions and degrade table lookup times to $O(N)$), Cachedb initializes a per-process
//! random 64-bit seed using [`std::hash::RandomState`]. A caller may also provide a fixed seed for
//! deterministic unit tests and reproducible benchmarks.

use std::hash::{BuildHasher, RandomState};
use std::sync::OnceLock;

/// Constant pseudorandom secrets derived from prime square roots (Wyhash constants).
const SECRET: [u64; 4] = [
    0xa076_1d64_78bd_642f,
    0xe703_7ed1_a0b4_28db,
    0x8ebc_6af0_9c88_c6e3,
    0x5899_65cc_c1b7_41f8,
];

/// Global per-process random seed initialized once from the standard library.
static PROCESS_SEED: OnceLock<u64> = OnceLock::new();

/// Returns the global per-process random seed.
///
/// The seed is lazily initialized using [`std::hash::RandomState`] on first access.
#[must_use]
pub fn process_seed() -> u64 {
    *PROCESS_SEED.get_or_init(|| {
        let state = RandomState::new();
        state.hash_one(0xCAFE_BABE_DEAD_BEEF_u64)
    })
}

/// 128-bit multiply and fold (`mum` primitive).
///
/// Multiplies two 64-bit integers into a 128-bit product, then XORs the upper
/// 64 bits with the lower 64 bits to produce a thoroughly mixed 64-bit result.
#[inline]
fn mum(a: u64, b: u64) -> u64 {
    let product = u128::from(a).wrapping_mul(u128::from(b));
    let bytes = product.to_le_bytes();
    let lo = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let hi = u64::from_le_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    lo ^ hi
}

/// Hashes an arbitrary byte slice using the given 64-bit seed.
///
/// # Panics
///
/// This function never panics. All arithmetic uses wrapping operations and bounds-safe chunking.
///
/// # Examples
///
/// ```
/// use cachedb::store::hash::hash64;
///
/// let h1 = hash64(b"hello", 42);
/// let h2 = hash64(b"hello", 42);
/// assert_eq!(h1, h2);
/// ```
#[must_use]
pub fn hash64(data: &[u8], seed: u64) -> u64 {
    let len = u64::try_from(data.len()).unwrap_or(u64::MAX);
    let mut a = seed ^ SECRET[0];
    let mut b = seed ^ SECRET[1];

    let chunks = data.chunks_exact(8);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(chunk);
        let word = u64::from_le_bytes(arr);
        a = mum(a ^ word, SECRET[2]);
        b = mum(b ^ word, SECRET[3]);
    }

    if !remainder.is_empty() {
        let mut tail = [0u8; 8];
        if let Some(dest) = tail.get_mut(..remainder.len()) {
            dest.copy_from_slice(remainder);
        }
        let tail_word = u64::from_le_bytes(tail);
        let rem_len = u64::try_from(remainder.len()).unwrap_or(0);
        a = mum(a ^ tail_word, SECRET[1] ^ rem_len);
    }

    let mixed = mum(a ^ len, b ^ SECRET[0]);
    mum(mixed, SECRET[1])
}

/// Hashes an arbitrary byte slice using the per-process random seed.
#[must_use]
pub fn hash64_process(data: &[u8]) -> u64 {
    hash64(data, process_seed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_input_same_seed_and_different_seed() {
        let input = b"cachedb-key-test";
        let h1 = hash64(input, 12345);
        let h2 = hash64(input, 12345);
        assert_eq!(h1, h2, "same input and same seed must yield same hash");

        let h3 = hash64(input, 54321);
        assert_ne!(h1, h3, "different seed must yield different hash");

        // Set of inputs across different seeds
        let sample_keys: [&[u8]; 5] = [b"a", b"alpha", b"user:100", b"session", b"counter"];
        for key in sample_keys {
            assert_eq!(hash64(key, 999), hash64(key, 999));
            assert_ne!(hash64(key, 111), hash64(key, 222));
        }
    }

    #[test]
    fn test_lengths_zero_to_64_no_panic() {
        let buffer = [0x5Au8; 64];
        for len in 0..=64 {
            let slice = &buffer[..len];
            let _ = hash64(slice, 42);
        }
    }

    #[test]
    fn test_inputs_differing_only_in_length_hash_differently() {
        let h_a = hash64(b"a", 100);
        let h_a_null = hash64(b"a\0", 100);
        assert_ne!(
            h_a, h_a_null,
            "inputs 'a' and 'a\\0' must produce distinct hashes"
        );

        let h_empty = hash64(b"", 100);
        let h_null = hash64(b"\0", 100);
        assert_ne!(
            h_empty, h_null,
            "empty string and single null byte must differ"
        );

        let h_short = hash64(b"abcdefgh", 100);
        let h_long = hash64(b"abcdefgh\0", 100);
        assert_ne!(h_short, h_long);
    }

    #[test]
    fn test_avalanche_property_flips_approx_half_bits() {
        let mut total_diff_bits = 0usize;
        let mut total_checks = 0usize;

        // Test over 200 distinct 8-byte inputs
        for i in 0..200u64 {
            let mut input = [0u8; 8];
            for (idx, byte) in input.iter_mut().enumerate() {
                *byte = ((i.wrapping_mul(37)).wrapping_add((idx as u64).wrapping_mul(13))) as u8;
            }

            let base_hash = hash64(&input, 0x1234_5678_9ABC_DEF0);

            // Flip each of the 64 bits one by one
            for bit in 0..64 {
                let mut flipped = input;
                let byte_idx = bit / 8;
                let bit_idx = bit % 8;
                flipped[byte_idx] ^= 1 << bit_idx;

                let new_hash = hash64(&flipped, 0x1234_5678_9ABC_DEF0);
                let diff = base_hash ^ new_hash;
                total_diff_bits += diff.count_ones() as usize;
                total_checks += 1;
            }
        }

        let avg_flipped = (total_diff_bits as f64) / (total_checks as f64);
        // Average flipped bits must be between 28 and 36 (ideal 32)
        assert!(
            (28.0..=36.0).contains(&avg_flipped),
            "avalanche test failed: avg flipped bits was {avg_flipped}, expected 28..=36"
        );
    }

    #[test]
    fn test_distribution_100k_keys_into_1024_buckets() {
        let mut buckets = vec![0usize; 1024];
        let total_keys = 100_000usize;
        let seed = 0xCAFE_BABE_DEAD_BEEF;

        for i in 0..total_keys {
            let key = format!("key-{i}");
            let h = hash64(key.as_bytes(), seed);
            let bucket = (h & 1023) as usize;
            buckets[bucket] += 1;
        }

        let expected_avg = (total_keys as f64) / 1024.0;
        let min_allowed = (expected_avg * 0.60) as usize; // -40%
        let max_allowed = (expected_avg * 1.40) as usize; // +40%

        for (bucket_idx, &count) in buckets.iter().enumerate() {
            assert!(
                count >= min_allowed && count <= max_allowed,
                "bucket {bucket_idx} count {count} outside +/-40% range ({min_allowed}..={max_allowed})"
            );
        }
    }
}
