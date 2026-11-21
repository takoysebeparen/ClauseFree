//! Deterministic data-parallel stages.
//!
//! A stage computes a result per item on many threads and returns the results **in item order**. The merge back into
//! the world then happens on one thread, in that order. So the outcome does not depend on how many threads ran or
//! how the work was scheduled, as long as the per-item function is pure: it may read shared data, but it must not
//! write anything shared (Rust's `Fn + Sync` bound enforces that at compile time, except for interior mutability,
//! which stage code must not use).
//!
//! Randomness inside a stage comes from [`map_with_rng`]: one seed is drawn from the world generator, and item `i`
//! gets its own generator on stream `i`. Which thread runs item `i` then makes no difference.
//!
//! ```
//! use sim::{parallel, rng::Rng};
//!
//! let items: Vec<u32> = (0..1000).collect();
//! let mut world_rng = Rng::new(1, 0);
//! let rolls = parallel::map_with_rng(&items, &mut world_rng, |item, rng| item + rng.below(6));
//! assert_eq!(rolls.len(), 1000);
//! ```

use std::fmt;

use rayon::prelude::*;

use crate::rng::Rng;

/// Applies `f` to every item in parallel; results are in item order.
pub fn map<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(usize, &T) -> R + Sync,
{
    items.par_iter().enumerate().map(|(i, item)| f(i, item)).collect()
}

/// Like [`map`], with a generator per item derived from `rng` (which advances by exactly two draws, however many
/// items there are).
pub fn map_with_rng<T, R, F>(items: &[T], rng: &mut Rng, f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T, &mut Rng) -> R + Sync,
{
    let seed = (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32());
    map(items, |i, item| {
        let mut item_rng = Rng::new(seed, i as u64);
        f(item, &mut item_rng)
    })
}

/// A thread pool could not be created.
#[derive(Debug)]
pub struct PoolError(rayon::ThreadPoolBuildError);

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot create thread pool: {}", self.0)
    }
}

impl std::error::Error for PoolError {}

/// Runs `f` with parallel stages limited to `threads` threads (0 means rayon's default, one per core). For tests
/// and for the host's settings; the result of the simulation must not depend on it.
pub fn with_threads<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> Result<R, PoolError> {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().map_err(PoolError)?;
    Ok(pool.install(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(items: &[u32], seed: u64) -> (Vec<u64>, Rng) {
        let mut rng = Rng::new(seed, 0);
        let out = map_with_rng(items, &mut rng, |item, r| {
            // Uneven work per item, so threads finish in a different order than they started.
            let mut acc = u64::from(*item);
            for _ in 0..(item % 97) {
                acc = acc.wrapping_mul(31).wrapping_add(u64::from(r.next_u32()));
            }
            acc
        });
        (out, rng)
    }

    #[test]
    fn same_result_for_any_thread_count() {
        let items: Vec<u32> = (0..5000).collect();
        let reference = with_threads(1, || stage(&items, 9)).unwrap_or_else(|e| panic!("{e}"));
        for threads in [2, 3, 8, 16] {
            let got = with_threads(threads, || stage(&items, 9)).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(got, reference, "{threads} threads");
        }
    }

    #[test]
    fn matches_a_sequential_loop() {
        let items: Vec<u32> = (0..300).collect();
        let (parallel, _) = stage(&items, 4);
        let mut rng = Rng::new(4, 0);
        let seed = (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32());
        let sequential: Vec<u64> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let mut r = Rng::new(seed, i as u64);
                let mut acc = u64::from(*item);
                for _ in 0..(item % 97) {
                    acc = acc.wrapping_mul(31).wrapping_add(u64::from(r.next_u32()));
                }
                acc
            })
            .collect();
        assert_eq!(parallel, sequential);
    }
}
