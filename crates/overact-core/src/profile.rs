//! Exact activity counts per minute for a set of persons.

use crate::dataset::Dataset;
use crate::format::{seg_code, seg_start};

/// Accumulates, in steps, how many persons do each activity at each minute.
pub struct Profiler {
    len: usize,
    n_act: usize,
    diff: Vec<i64>,
    pos: usize,
}

impl Profiler {
    pub fn new(ds: &Dataset) -> Profiler {
        let len = ds.axis_len() as usize;
        let n_act = ds.n_activities();
        Profiler {
            len,
            n_act,
            diff: vec![0; n_act * (len + 1)],
            pos: 0,
        }
    }

    /// Processes up to `budget` persons from `rows`. Returns true when done.
    pub fn step(&mut self, ds: &Dataset, rows: &[u32], budget: usize) -> bool {
        let end = (self.pos + budget.max(1)).min(rows.len());
        let chunk = &rows[self.pos..end];
        #[cfg(feature = "parallel")]
        if chunk.len() >= 1 << 16 && rayon::current_num_threads() > 1 {
            use rayon::prelude::*;
            let size = self.diff.len();
            let len = self.len;
            let sum = chunk
                .par_chunks(1 << 15)
                .fold(
                    || vec![0i64; size],
                    |mut d, c| {
                        accumulate(ds, c, len, &mut d);
                        d
                    },
                )
                .reduce(
                    || vec![0i64; size],
                    |mut a, b| {
                        a.iter_mut().zip(&b).for_each(|(x, y)| *x += y);
                        a
                    },
                );
            self.diff.iter_mut().zip(&sum).for_each(|(x, y)| *x += y);
            self.pos = end;
            return end == rows.len();
        }
        accumulate(ds, chunk, self.len, &mut self.diff);
        self.pos = end;
        end == rows.len()
    }

    /// Counts as `counts[code * len + minute]`.
    pub fn finish(self) -> Vec<u32> {
        let mut out = vec![0u32; self.n_act * self.len];
        let stride = self.len + 1;
        for c in 0..self.n_act {
            let mut run = 0i64;
            for t in 0..self.len {
                run += self.diff[c * stride + t];
                out[c * self.len + t] = run as u32;
            }
        }
        out
    }
}

/// Adds each person's segments to the per-activity difference arrays.
fn accumulate(ds: &Dataset, rows: &[u32], len: usize, diff: &mut [i64]) {
    let stride = len + 1;
    for &p in rows {
        let segs = ds.person(p as usize);
        for (k, &s) in segs.iter().enumerate() {
            let a = seg_start(s) as usize;
            let b = segs.get(k + 1).map_or(len, |&n| seg_start(n) as usize);
            let base = seg_code(s) as usize * stride;
            diff[base + a] += 1;
            diff[base + b] -= 1;
        }
    }
}

pub fn profile(ds: &Dataset, rows: &[u32]) -> Vec<u32> {
    let mut p = Profiler::new(ds);
    p.step(ds, rows, rows.len());
    p.finish()
}
