//! Stable LSD radix sort of a row list by several keys, run in small steps so
//! a host can interleave other work and cancel.
//!
//! Each key maps to a monotone unsigned key (`Column::sort_key`), split into
//! 32-bit words that are sorted with 16-bit digits, least significant first.
//! The input is in ascending person order and the sort is stable, so ties end
//! up in person order. Missing values sort last in both directions.

use crate::column::{Column, NULL_KEY};
use crate::filter::ColRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SortKey {
    pub col: ColRef,
    #[serde(default)]
    pub desc: bool,
    /// Optional rank for each category index (custom category order).
    #[serde(default)]
    pub order: Option<Vec<u32>>,
}

/// One 32-bit word of one key.
#[derive(Debug, Clone, Copy)]
struct Word {
    key: usize,
    shift: u32,
    digits: u32,
}

#[derive(Debug, Clone, Copy)]
enum Stage {
    Gather { word: usize, pos: usize },
    Hist { word: usize, digit: u32, pos: usize },
    Scatter { word: usize, digit: u32, pos: usize },
    Done,
}

pub struct RadixSorter {
    words: Vec<Word>,
    desc: Vec<bool>,
    bits: Vec<u32>,
    order: Vec<Option<Vec<u32>>>,
    pub rows: Vec<u32>,
    tmp_rows: Vec<u32>,
    keys: Vec<u32>,
    tmp_keys: Vec<u32>,
    hist: Vec<u32>,
    stage: Stage,
}

/// Monotone key for `p`, with direction and custom category order applied.
/// Non-null keys are below `2^bits - 1`; nulls are exactly `2^bits - 1`.
#[inline]
fn key_of(col: &Column, p: usize, bits: u32, desc: bool, order: Option<&Vec<u32>>) -> u64 {
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let mut k = col.sort_key(p);
    if k == NULL_KEY {
        return mask;
    }
    if let Some(o) = order {
        // Ranks are capped below the null key (u32::MAX with 32 bits).
        k = o
            .get(k as usize)
            .map_or(u32::MAX - 1, |&r| r.min(u32::MAX - 1)) as u64;
    }
    if desc {
        mask - 1 - k
    } else {
        k
    }
}

/// Fills `keys` with word `shift` of each row's key, with a tight loop per
/// column type (the common case without a custom category order).
fn gather(col: &Column, rows: &[u32], keys: &mut [u32], bits: u32, desc: bool, shift: u32) {
    use crate::column::{f32_key, f64_key, ColumnData as D};
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    #[inline(always)]
    fn run<T: Copy>(
        data: &[T],
        rows: &[u32],
        keys: &mut [u32],
        mask: u64,
        desc: bool,
        shift: u32,
        key: impl Fn(T) -> Option<u64>,
    ) {
        for (k, &p) in keys.iter_mut().zip(rows) {
            let v = match key(data[p as usize]) {
                None => mask,
                Some(x) if desc => mask - 1 - x,
                Some(x) => x,
            };
            *k = (v >> shift) as u32;
        }
    }
    let null = col.meta.null_value;
    match &col.data {
        D::U8(d) => {
            let nv = null.map(|v| v as u8);
            run(d, rows, keys, mask, desc, shift, |v| {
                (Some(v) != nv).then_some(v as u64)
            })
        }
        D::U16(d) => {
            let nv = null.map(|v| v as u16);
            run(d, rows, keys, mask, desc, shift, |v| {
                (Some(v) != nv).then_some(v as u64)
            })
        }
        D::I8(d) => run(d, rows, keys, mask, desc, shift, |v| {
            (v != i8::MIN).then(|| (v as i64 + 127) as u64)
        }),
        D::I16(d) => run(d, rows, keys, mask, desc, shift, |v| {
            (v != i16::MIN).then(|| (v as i64 + 32767) as u64)
        }),
        D::I32(d) => run(d, rows, keys, mask, desc, shift, |v| {
            (v != i32::MIN).then(|| (v as i64 + 2147483647) as u64)
        }),
        D::F32(d) => run(d, rows, keys, mask, desc, shift, |v| {
            f32_key(v).map(|k| k as u64)
        }),
        D::F64(d) => run(d, rows, keys, mask, desc, shift, f64_key),
    }
}

impl RadixSorter {
    /// `rows` must be in ascending order (for person-index tiebreaks).
    pub fn new(rows: Vec<u32>, keys: &[SortKey], cols: &[&Column]) -> RadixSorter {
        let mut words = Vec::new();
        let bits: Vec<u32> = keys
            .iter()
            .zip(cols)
            .map(|(k, c)| if k.order.is_some() { 32 } else { c.key_bits() })
            .collect();
        for i in (0..keys.len()).rev() {
            let b = bits[i];
            let n_words = b.div_ceil(32);
            for w in 0..n_words {
                let digits = if b == 16 { 1 } else { 2 };
                words.push(Word {
                    key: i,
                    shift: 32 * w,
                    digits,
                });
            }
        }
        let n = rows.len();
        let stage = if words.is_empty() || n < 2 {
            Stage::Done
        } else {
            Stage::Gather { word: 0, pos: 0 }
        };
        RadixSorter {
            words,
            desc: keys.iter().map(|k| k.desc).collect(),
            bits,
            order: keys.iter().map(|k| k.order.clone()).collect(),
            tmp_rows: vec![0; if matches!(stage, Stage::Done) { 0 } else { n }],
            keys: vec![0; if matches!(stage, Stage::Done) { 0 } else { n }],
            tmp_keys: vec![0; if matches!(stage, Stage::Done) { 0 } else { n }],
            hist: vec![0; 1 << 16],
            rows,
            stage,
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }

    /// Does up to about `budget` element operations. Returns true when done.
    pub fn step(&mut self, cols: &[&Column], budget: usize) -> bool {
        #[cfg(feature = "parallel")]
        if self.rows.len() >= PAR_MIN && rayon::current_num_threads() > 1 {
            return self.step_parallel(cols);
        }
        let n = self.rows.len();
        let mut left = budget.max(1);
        while left > 0 {
            match self.stage {
                Stage::Done => return true,
                Stage::Gather { word, pos } => {
                    let w = self.words[word];
                    let (col, bits, desc) = (cols[w.key], self.bits[w.key], self.desc[w.key]);
                    let order = self.order[w.key].as_ref();
                    let end = (pos + left).min(n);
                    let (rows, keys) = (&self.rows[pos..end], &mut self.keys[pos..end]);
                    if order.is_some() {
                        for (k, &p) in keys.iter_mut().zip(rows) {
                            *k = (key_of(col, p as usize, bits, desc, order) >> w.shift) as u32;
                        }
                    } else {
                        gather(col, rows, keys, bits, desc, w.shift);
                    }
                    left -= end - pos;
                    self.stage = if end == n {
                        self.hist.iter_mut().for_each(|h| *h = 0);
                        Stage::Hist {
                            word,
                            digit: 0,
                            pos: 0,
                        }
                    } else {
                        Stage::Gather { word, pos: end }
                    };
                }
                Stage::Hist { word, digit, pos } => {
                    let end = (pos + left).min(n);
                    let sh = 16 * digit;
                    for &k in &self.keys[pos..end] {
                        self.hist[((k >> sh) & 0xffff) as usize] += 1;
                    }
                    left -= end - pos;
                    if end < n {
                        self.stage = Stage::Hist {
                            word,
                            digit,
                            pos: end,
                        };
                        continue;
                    }
                    // All keys share this digit: the pass would not move anything.
                    if self.hist.iter().any(|&h| h as usize == n) {
                        self.stage = self.next_digit(word, digit);
                        continue;
                    }
                    let mut sum = 0u32;
                    for h in self.hist.iter_mut() {
                        let c = *h;
                        *h = sum;
                        sum += c;
                    }
                    self.stage = Stage::Scatter {
                        word,
                        digit,
                        pos: 0,
                    };
                }
                Stage::Scatter { word, digit, pos } => {
                    let end = (pos + left).min(n);
                    let sh = 16 * digit;
                    for j in pos..end {
                        let k = self.keys[j];
                        let d = ((k >> sh) & 0xffff) as usize;
                        let o = self.hist[d] as usize;
                        self.hist[d] += 1;
                        self.tmp_keys[o] = k;
                        self.tmp_rows[o] = self.rows[j];
                    }
                    left -= end - pos;
                    if end < n {
                        self.stage = Stage::Scatter {
                            word,
                            digit,
                            pos: end,
                        };
                        continue;
                    }
                    std::mem::swap(&mut self.keys, &mut self.tmp_keys);
                    std::mem::swap(&mut self.rows, &mut self.tmp_rows);
                    self.stage = self.next_digit(word, digit);
                }
            }
        }
        self.is_done()
    }

    /// Runs one whole stage (a gather, or one digit pass) using all threads.
    #[cfg(feature = "parallel")]
    fn step_parallel(&mut self, cols: &[&Column]) -> bool {
        use rayon::prelude::*;
        match self.stage {
            Stage::Done => {}
            Stage::Gather { word, .. } => {
                let w = self.words[word];
                let (col, bits, desc) = (cols[w.key], self.bits[w.key], self.desc[w.key]);
                let order = self.order[w.key].as_ref();
                self.keys
                    .par_chunks_mut(PAR_CHUNK)
                    .zip(self.rows.par_chunks(PAR_CHUNK))
                    .for_each(|(keys, rows)| {
                        if order.is_some() {
                            for (k, &p) in keys.iter_mut().zip(rows) {
                                *k = (key_of(col, p as usize, bits, desc, order) >> w.shift) as u32;
                            }
                        } else {
                            gather(col, rows, keys, bits, desc, w.shift);
                        }
                    });
                self.stage = Stage::Hist {
                    word,
                    digit: 0,
                    pos: 0,
                };
            }
            Stage::Hist { word, digit, .. } | Stage::Scatter { word, digit, .. } => {
                if parallel_pass(
                    &mut self.keys,
                    &mut self.rows,
                    &mut self.tmp_keys,
                    &mut self.tmp_rows,
                    16 * digit,
                ) {
                    std::mem::swap(&mut self.keys, &mut self.tmp_keys);
                    std::mem::swap(&mut self.rows, &mut self.tmp_rows);
                }
                self.stage = self.next_digit(word, digit);
            }
        }
        self.is_done()
    }

    fn next_digit(&mut self, word: usize, digit: u32) -> Stage {
        if digit + 1 < self.words[word].digits {
            self.hist.iter_mut().for_each(|h| *h = 0);
            Stage::Hist {
                word,
                digit: digit + 1,
                pos: 0,
            }
        } else if word + 1 < self.words.len() {
            Stage::Gather {
                word: word + 1,
                pos: 0,
            }
        } else {
            // Free scratch memory early.
            self.keys = Vec::new();
            self.tmp_keys = Vec::new();
            self.tmp_rows = Vec::new();
            Stage::Done
        }
    }

    pub fn into_rows(self) -> Vec<u32> {
        self.rows
    }
}

#[cfg(feature = "parallel")]
const PAR_MIN: usize = 1 << 18;
#[cfg(feature = "parallel")]
const PAR_CHUNK: usize = 1 << 16;

/// A raw pointer that may be shared across threads; each thread writes to
/// disjoint indices.
#[cfg(feature = "parallel")]
#[derive(Clone, Copy)]
struct SharedMut<T>(*mut T);
#[cfg(feature = "parallel")]
unsafe impl<T> Send for SharedMut<T> {}
#[cfg(feature = "parallel")]
unsafe impl<T> Sync for SharedMut<T> {}

/// One stable counting-sort pass on the 16-bit digit at `shift`, from
/// (keys, rows) into (tmp_keys, tmp_rows), using one contiguous chunk per
/// task. Returns false (moving nothing) when every key has the same digit.
#[cfg(feature = "parallel")]
fn parallel_pass(
    keys: &mut [u32],
    rows: &mut [u32],
    tmp_keys: &mut [u32],
    tmp_rows: &mut [u32],
    shift: u32,
) -> bool {
    use rayon::prelude::*;
    let n = keys.len();
    let tasks = (rayon::current_num_threads() * 2).clamp(1, n.div_ceil(PAR_CHUNK).max(1));
    let size = n.div_ceil(tasks);
    // Per-chunk digit histograms.
    let mut hists: Vec<Vec<u32>> = keys
        .par_chunks(size)
        .map(|c| {
            let mut h = vec![0u32; 1 << 16];
            for &k in c {
                h[((k >> shift) & 0xffff) as usize] += 1;
            }
            h
        })
        .collect();
    let mut total = vec![0u64; 1 << 16];
    for h in &hists {
        for (t, &c) in total.iter_mut().zip(h) {
            *t += c as u64;
        }
    }
    if total.iter().any(|&t| t as usize == n) {
        return false;
    }
    // Exclusive offsets: digit-major, then chunk order (keeps the pass stable).
    let mut sum = 0u32;
    for d in 0..1 << 16 {
        for h in hists.iter_mut() {
            let c = h[d];
            h[d] = sum;
            sum += c;
        }
    }
    let (pk, pr) = (
        SharedMut(tmp_keys.as_mut_ptr()),
        SharedMut(tmp_rows.as_mut_ptr()),
    );
    keys.par_chunks(size)
        .zip(rows.par_chunks(size))
        .zip(hists.par_iter_mut())
        .for_each(|((kc, rc), off)| {
            let (pk, pr) = (pk, pr);
            for (&k, &r) in kc.iter().zip(rc) {
                let d = ((k >> shift) & 0xffff) as usize;
                let o = off[d] as usize;
                off[d] += 1;
                // SAFETY: offsets partition 0..n, so every o is written exactly
                // once, by exactly one task; both buffers have length n.
                unsafe {
                    *pk.0.add(o) = k;
                    *pr.0.add(o) = r;
                }
            }
        });
    true
}

/// Convenience: sort to completion.
pub fn sort_rows(rows: Vec<u32>, keys: &[SortKey], cols: &[&Column]) -> Vec<u32> {
    let mut s = RadixSorter::new(rows, keys, cols);
    while !s.step(cols, usize::MAX / 2) {}
    s.into_rows()
}
