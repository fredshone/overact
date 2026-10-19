//! Random subsamples of a row list.
//!
//! A sample of `n` keeps the `n` rows with the smallest `rank` (bottom-k by a
//! per-person hash). Whether a person is kept depends only on the seed, the
//! other persons in the list and `n`, so a larger sample contains every
//! smaller one, and a person shared by two windows tends to be kept in both.
//!
//! `Sampler` does this in steps: it collects the rows whose hash is below a
//! threshold chosen to keep slightly more than `n` rows on average, then
//! selects exactly `n` of those candidates. If too few pass (rare), it
//! rescans with a higher threshold.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// Rows to keep (all of them if the list is no longer).
    pub n: u32,
    #[serde(default)]
    pub seed: u32,
}

/// A total order on persons for a seed: a 32-bit hash, ties broken by index.
#[inline]
pub fn rank(p: u32, seed: u32) -> u64 {
    // splitmix64 finaliser.
    let mut x = ((seed as u64) << 32 | p as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x & 0xffff_ffff_0000_0000) | p as u64
}

pub struct Sampler {
    n: usize,
    seed: u32,
    /// Rows whose rank is below this pass the scan.
    threshold: u64,
    pos: usize,
    cand: Vec<u32>,
    done: bool,
}

impl Sampler {
    /// Samples `n` of the `len` rows of a list (`n < len`).
    pub fn new(len: usize, n: usize, seed: u32) -> Sampler {
        // Expect a few standard deviations more candidates than needed.
        let want = n as f64 + 4.0 * (n as f64).sqrt() + 16.0;
        Sampler {
            n,
            seed,
            threshold: threshold(want / len.max(1) as f64),
            pos: 0,
            cand: Vec::with_capacity(want.min(len as f64) as usize),
            done: n == 0,
        }
    }

    /// Scans about `budget` rows of `rows`. Returns true when done.
    pub fn step(&mut self, rows: &[u32], budget: usize) -> bool {
        if self.done {
            return true;
        }
        let end = (self.pos + budget.max(1)).min(rows.len());
        let (t, seed) = (self.threshold, self.seed);
        self.cand
            .extend(rows[self.pos..end].iter().filter(|&&p| rank(p, seed) < t));
        self.pos = end;
        if end < rows.len() {
            return false;
        }
        if self.cand.len() < self.n {
            // Unlucky: rescan with a higher threshold.
            self.threshold = self.threshold.saturating_mul(2);
            self.cand.clear();
            self.pos = 0;
            return false;
        }
        if self.cand.len() > self.n {
            let seed = self.seed;
            self.cand
                .select_nth_unstable_by_key(self.n - 1, |&p| rank(p, seed));
            self.cand.truncate(self.n);
        }
        self.cand.sort_unstable();
        self.cand.shrink_to_fit();
        self.done = true;
        true
    }

    /// The sampled rows, ascending.
    pub fn into_rows(self) -> Vec<u32> {
        debug_assert!(self.done);
        if self.n == 0 {
            Vec::new()
        } else {
            self.cand
        }
    }
}

/// The rank threshold that passes about `share` of all rows.
fn threshold(share: f64) -> u64 {
    // Ranks keep the hash in the high 32 bits.
    let h = (share * 4294967296.0).ceil() as u64;
    if h >= 1 << 32 {
        u64::MAX
    } else {
        h.max(1) << 32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn naive(rows: &[u32], n: usize, seed: u32) -> Vec<u32> {
        let mut r = rows.to_vec();
        r.sort_by_key(|&p| rank(p, seed));
        r.truncate(n);
        r.sort_unstable();
        r
    }

    fn run(rows: &[u32], n: usize, seed: u32, budget: usize) -> Vec<u32> {
        let mut s = Sampler::new(rows.len(), n, seed);
        while !s.step(rows, budget) {}
        s.into_rows()
    }

    proptest! {
        #[test]
        fn sampler_matches_naive(
            mut rows in proptest::collection::vec(0u32..5000, 0..800),
            n in 0usize..900,
            seed in any::<u32>(),
            budget in prop_oneof![1usize..16, Just(1 << 20)],
        ) {
            rows.sort_unstable();
            rows.dedup();
            let n = n.min(rows.len().saturating_sub(1));
            prop_assert_eq!(run(&rows, n, seed, budget), naive(&rows, n, seed));
        }
    }

    #[test]
    fn larger_samples_contain_smaller() {
        let rows: Vec<u32> = (0..10_000).collect();
        let small = run(&rows, 100, 3, 1 << 20);
        let big = run(&rows, 1000, 3, 1 << 20);
        assert!(small.iter().all(|p| big.binary_search(p).is_ok()));
        assert_ne!(small, run(&rows, 100, 4, 1 << 20));
    }

    #[test]
    fn rescans_when_threshold_is_too_low() {
        let rows: Vec<u32> = (0..2000).collect();
        let mut s = Sampler::new(rows.len(), 500, 1);
        s.threshold = 1 << 32; // passes almost nothing
        while !s.step(&rows, 64) {}
        assert_eq!(s.into_rows(), naive(&rows, 500, 1));
    }
}
