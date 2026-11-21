//! A stable hash of the world state, for desync detection and reproducibility tests.
//!
//! The standard library's hashers are not guaranteed to be stable across Rust versions or platforms, so the state
//! hash uses **FNV-1a 64** (<http://www.isthe.com/chongo/tech/comp/fnv/>), fed with explicitly ordered,
//! little-endian bytes. It is not cryptographic; it only has to differ when the state differs. Every value hashes
//! its fields in a fixed order and every collection hashes its length first, so `[a, b] + [c]` and `[a] + [b, c]`
//! differ.

use std::collections::BTreeMap;

use common::Fixed;
use data::Tag;
use data::states::StateId;

use crate::time::GameTime;

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64 over a stream of bytes.
#[derive(Debug, Clone)]
pub struct StateHasher(u64);

impl Default for StateHasher {
    fn default() -> Self {
        StateHasher(OFFSET_BASIS)
    }
}

impl StateHasher {
    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }

    pub fn write_u8(&mut self, v: u8) {
        self.write(&[v]);
    }

    pub fn write_u32(&mut self, v: u32) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.write(&v.to_le_bytes());
    }

    /// Lengths are hashed as `u64` so the result does not depend on the platform's pointer size.
    pub fn write_len(&mut self, len: usize) {
        self.write_u64(len as u64);
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Values that contribute to the world hash.
pub trait StateHash {
    fn hash_into(&self, h: &mut StateHasher);
}

/// The hash of a single value.
pub fn hash_of<T: StateHash + ?Sized>(value: &T) -> u64 {
    let mut h = StateHasher::default();
    value.hash_into(&mut h);
    h.finish()
}

impl StateHash for u32 {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_u32(*self);
    }
}

impl StateHash for u64 {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_u64(*self);
    }
}

impl StateHash for i64 {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_i64(*self);
    }
}

impl StateHash for bool {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_u8(u8::from(*self));
    }
}

impl StateHash for Fixed {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_i64(self.raw());
    }
}

impl StateHash for Tag {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write(self.as_str().as_bytes());
    }
}

impl StateHash for StateId {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_u32(self.0);
    }
}

impl StateHash for GameTime {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_i64(self.0);
    }
}

impl<T: StateHash> StateHash for Option<T> {
    fn hash_into(&self, h: &mut StateHasher) {
        match self {
            None => h.write_u8(0),
            Some(v) => {
                h.write_u8(1);
                v.hash_into(h);
            }
        }
    }
}

impl<T: StateHash> StateHash for [T] {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_len(self.len());
        for v in self {
            v.hash_into(h);
        }
    }
}

impl<T: StateHash> StateHash for Vec<T> {
    fn hash_into(&self, h: &mut StateHasher) {
        self.as_slice().hash_into(h);
    }
}

impl<K: StateHash, V: StateHash> StateHash for BTreeMap<K, V> {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_len(self.len());
        for (k, v) in self {
            k.hash_into(h);
            v.hash_into(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published FNV-1a 64 test vectors.
    #[test]
    fn fnv1a_vectors() {
        let fnv = |s: &str| {
            let mut h = StateHasher::default();
            h.write(s.as_bytes());
            h.finish()
        };
        assert_eq!(fnv(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv("foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn collections_are_length_prefixed() {
        let a = (vec![1u32, 2], vec![3u32]);
        let b = (vec![1u32], vec![2u32, 3]);
        let hash = |(x, y): &(Vec<u32>, Vec<u32>)| {
            let mut h = StateHasher::default();
            x.hash_into(&mut h);
            y.hash_into(&mut h);
            h.finish()
        };
        assert_ne!(hash(&a), hash(&b));
        assert_ne!(hash_of(&None::<u32>), hash_of(&Some(0u32)));
    }
}
