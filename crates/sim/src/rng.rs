//! The simulation's random number generator.
//!
//! The original's generator is unknown and is not something we can or want to copy (clean room); ClauseFree aims to
//! be indistinguishable, not bit-identical, so any good deterministic generator will do. We use **PCG32**
//! (`pcg32`, "XSH RR" output, 64-bit state, selectable stream) by M. E. O'Neill, <https://www.pcg-random.org>,
//! because it is small, fast, statistically strong, fully specified, and its stream parameter lets independent
//! subsystems draw from independent sequences without sharing state (needed for deterministic parallel stages).
//!
//! The generator is part of the world state: it is hashed and saved, and it advances only when the simulation draws
//! from it. It is never seeded from time or from a hash of memory addresses.

use common::Fixed;

use crate::hash::{StateHash, StateHasher};

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// A PCG32 generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: u64,
    increment: u64,
}

impl Rng {
    /// Seeds a generator; `stream` selects one of 2^63 independent sequences. Same as `pcg32_srandom_r` of the
    /// reference implementation.
    pub fn new(seed: u64, stream: u64) -> Rng {
        let mut rng = Rng { state: 0, increment: (stream << 1) | 1 };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// The next uniformly distributed 32-bit value.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.increment);
        // Truncations are part of the algorithm's definition.
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rotation = (old >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    /// A uniformly distributed value in `0..bound`, without modulo bias (`pcg32_boundedrand_r`). Returns 0 when
    /// `bound` is 0.
    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % bound;
            }
        }
    }

    /// `true` with probability `p` (clamped to 0..=1), at the resolution of [`Fixed`].
    pub fn chance(&mut self, p: Fixed) -> bool {
        let scale = Fixed::SCALE as u32; // 100 000 fits in u32.
        let raw = p.raw().clamp(0, Fixed::SCALE) as u32;
        self.below(scale) < raw
    }

    /// An independent generator for a subsystem, derived deterministically from this one (advances this one by
    /// two draws).
    pub fn fork(&mut self, stream: u64) -> Rng {
        let seed = (u64::from(self.next_u32()) << 32) | u64::from(self.next_u32());
        Rng::new(seed, stream)
    }
}

impl StateHash for Rng {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_u64(self.state);
        h.write_u64(self.increment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Output of the reference implementation's `pcg32-demo` (seed 42, stream 54).
    #[test]
    fn matches_reference_implementation() {
        let mut rng = Rng::new(42, 54);
        let got: Vec<u32> = (0..6).map(|_| rng.next_u32()).collect();
        assert_eq!(got, [0xa15c_02b7, 0x7b47_f409, 0xba1d_3330, 0x83d2_f293, 0xbfa4_784b, 0xcbed_606e]);
    }

    #[test]
    fn bounded_values_stay_in_range() {
        let mut rng = Rng::new(1, 1);
        for bound in [1u32, 2, 3, 7, 100, 1 << 31, u32::MAX] {
            for _ in 0..1000 {
                assert!(rng.below(bound) < bound);
            }
        }
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn chance_extremes() {
        let mut rng = Rng::new(7, 0);
        for _ in 0..1000 {
            assert!(!rng.chance(Fixed::ZERO));
            assert!(rng.chance(Fixed::ONE));
        }
    }

    #[test]
    fn streams_and_forks_differ() {
        let a: Vec<u32> = {
            let mut r = Rng::new(5, 0);
            (0..8).map(|_| r.next_u32()).collect()
        };
        let b: Vec<u32> = {
            let mut r = Rng::new(5, 1);
            (0..8).map(|_| r.next_u32()).collect()
        };
        assert_ne!(a, b);
        let mut parent = Rng::new(5, 0);
        let mut x = parent.clone().fork(3);
        let mut y = parent.fork(3);
        assert_eq!(x.next_u32(), y.next_u32());
    }
}
