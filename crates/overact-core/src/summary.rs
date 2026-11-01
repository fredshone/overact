//! Aggregate statistics of a set of persons, computed in one pass: activity
//! sequences, start × duration densities, participation, transitions and
//! n-grams.
//!
//! Everything counts *episodes*: back-to-back segments with the same code
//! merged into one. A person's *sequence* is the list of its episode codes.

use crate::dataset::Dataset;
use crate::format::{seg_code, seg_start};
use serde::Serialize;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// Most time bins along each axis of the start × duration density.
pub const MAX_BINS: usize = 48;
/// Episode counts per person are capped at this (the last bin is "or more").
pub const MAX_EPISODES: usize = 6;
/// Episodes per person histogram cap.
pub const MAX_LENGTH: usize = 32;
/// Sequences kept, most frequent first.
pub const TOP_SEQUENCES: usize = 256;
/// N-grams kept per n.
pub const TOP_NGRAMS: usize = 32;
/// Resolution of the grid that sums up the sequences after the top list:
/// rows down the rarer persons, columns across each sequence.
pub const TAIL_ROWS: usize = 128;
pub const TAIL_COLS: usize = 32;

/// Sequences longer than this are keyed by a hash instead of exactly.
const EXACT_LEN: usize = 24;

/// FxHash: fast and good enough for keys that are already well mixed.
#[derive(Default, Clone, Copy)]
struct Fx(u64);

impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u128(&mut self, x: u128) {
        self.write_u64(x as u64);
        self.write_u64((x >> 64) as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Counted {
    pub codes: Vec<u8>,
    pub count: u32,
}

/// The finished statistics. Arrays are flat, row-major, indexed by activity
/// code first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub n_persons: u32,
    pub n_activities: usize,
    /// Minutes per time bin, and the number of bins (`ceil(axis_len / bin)`).
    pub bin: u32,
    pub bins: usize,
    /// Most frequent sequences, by count descending, then by the first person
    /// (in index order) having them.
    pub sequences: Vec<Counted>,
    /// Distinct sequences in total.
    pub n_distinct: u32,
    /// The sequences after the top list, in the same order, as a grid
    /// `[row][col][code]` of `TAIL_ROWS × TAIL_COLS` cells. Row `r` covers
    /// the `r`-th 1/`TAIL_ROWS` of those persons; column `c` is the code at
    /// the middle of the `c`-th 1/`TAIL_COLS` of each sequence. Cells hold
    /// persons × `TAIL_ROWS` (so every non-empty row sums to the number of
    /// rarer persons in each column).
    pub tail: Vec<u32>,
    pub tail_rows: usize,
    pub tail_cols: usize,
    /// `[code][start bin][duration bin]` episode counts. Start bin `k` covers
    /// minutes `[k*bin, (k+1)*bin)`; duration bin `k` covers `(k*bin, (k+1)*bin]`.
    pub start_dur: Vec<u32>,
    /// `[code][k]`: persons with `k` episodes of the activity (`k` capped at `MAX_EPISODES`).
    pub episodes: Vec<u32>,
    /// `[code][bin]`: participants by total minutes in the activity (bins as durations).
    pub minutes: Vec<u32>,
    /// `[k]`: persons with `k` episodes in total (capped at `MAX_LENGTH`).
    pub n_episodes: Vec<u32>,
    /// `[from][to]` over `n_activities + 1` states, the last being the start
    /// (as `from`) or end (as `to`) of the day.
    pub trans: Vec<u32>,
    /// Most frequent 3- and 4-grams of episode codes (occurrences, not persons).
    pub ngrams: Vec<Counted>,
}

/// Accumulates a `Summary` in steps.
pub struct Summarizer {
    len: usize,
    n_act: usize,
    bin: usize,
    bins: usize,
    n_persons: u32,
    /// Sequence key -> (first person, count).
    sequences: FxMap<u128, (u32, u32)>,
    start_dur: Vec<u32>,
    episodes: Vec<u32>,
    minutes: Vec<u32>,
    n_episodes: Vec<u32>,
    trans: Vec<u32>,
    tri: Vec<u32>,
    quad: Vec<u32>,
    pos: usize,
    /// Time bin of each minute (division by `bin` is slow in the hot loop).
    bin_of: Vec<u8>,
    // Per-person scratch.
    eps: Vec<(u8, u16, u16)>,
    count: Vec<u32>,
    total: Vec<u32>,
    touched: Vec<u8>,
}

impl Summarizer {
    pub fn new(ds: &Dataset) -> Summarizer {
        let len = ds.axis_len() as usize;
        let n_act = ds.n_activities();
        let bin = len.div_ceil(MAX_BINS).max(1);
        let bins = len.div_ceil(bin).max(1);
        Summarizer {
            len,
            n_act,
            bin,
            bins,
            n_persons: 0,
            sequences: FxMap::default(),
            start_dur: vec![0; n_act * bins * bins],
            episodes: vec![0; n_act * (MAX_EPISODES + 1)],
            minutes: vec![0; n_act * bins],
            n_episodes: vec![0; MAX_LENGTH + 1],
            trans: vec![0; (n_act + 1) * (n_act + 1)],
            tri: vec![0; n_act.pow(3)],
            quad: vec![0; n_act.pow(4)],
            pos: 0,
            bin_of: (0..len.max(1))
                .map(|m| (m / bin).min(bins - 1) as u8)
                .collect(),
            eps: Vec::new(),
            count: vec![0; n_act],
            total: vec![0; n_act],
            touched: Vec::new(),
        }
    }

    /// Processes up to `budget` persons from `rows` (ascending). Returns true when done.
    pub fn step(&mut self, ds: &Dataset, rows: &[u32], budget: usize) -> bool {
        let end = (self.pos + budget.max(1)).min(rows.len());
        let chunk = &rows[self.pos..end];
        // Persons cost far more than in the profile, so the job's adaptive
        // steps are smaller: split from 2^14 persons on.
        #[cfg(feature = "parallel")]
        if chunk.len() >= 1 << 14 && rayon::current_num_threads() > 1 {
            use rayon::prelude::*;
            let sum = chunk
                .par_chunks(1 << 12)
                .fold(
                    || Summarizer::new(ds),
                    |mut s, c| {
                        c.iter().for_each(|&p| s.add(ds, p));
                        s
                    },
                )
                .reduce(
                    || Summarizer::new(ds),
                    |mut a, b| {
                        a.merge(b);
                        a
                    },
                );
            self.merge(sum);
            self.pos = end;
            return end == rows.len();
        }
        for &p in chunk {
            self.add(ds, p);
        }
        self.pos = end;
        end == rows.len()
    }

    /// Adds another summariser's counts (of other persons).
    pub fn merge(&mut self, o: Summarizer) {
        let add = |a: &mut Vec<u32>, b: &[u32]| a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
        add(&mut self.start_dur, &o.start_dur);
        add(&mut self.episodes, &o.episodes);
        add(&mut self.minutes, &o.minutes);
        add(&mut self.n_episodes, &o.n_episodes);
        add(&mut self.trans, &o.trans);
        add(&mut self.tri, &o.tri);
        add(&mut self.quad, &o.quad);
        self.n_persons += o.n_persons;
        for (k, (rep, n)) in o.sequences {
            let e = self.sequences.entry(k).or_insert((rep, 0));
            e.0 = e.0.min(rep);
            e.1 += n;
        }
    }

    fn add(&mut self, ds: &Dataset, p: u32) {
        episodes(ds, p as usize, self.len, &mut self.eps);
        let (na, bins) = (self.n_act, self.bins);
        let eps = &self.eps;
        let bin_of = &self.bin_of[..];
        // Bin of a duration or total (bin k covers (k*bin, (k+1)*bin]).
        let dur_bin = |d: usize| bin_of[d.saturating_sub(1).min(bin_of.len() - 1)] as usize;
        let mut prev = na;
        for (i, &(c, a, b)) in eps.iter().enumerate() {
            let c = c as usize;
            let dur = (b - a) as usize;
            let sb = bin_of[a as usize] as usize;
            self.start_dur[(c * bins + sb) * bins + dur_bin(dur)] += 1;
            if self.count[c] == 0 {
                self.touched.push(c as u8);
            }
            self.count[c] += 1;
            self.total[c] += dur as u32;
            self.trans[prev * (na + 1) + c] += 1;
            prev = c;
            if i >= 2 {
                let (x, y) = (eps[i - 2].0 as usize, eps[i - 1].0 as usize);
                self.tri[(x * na + y) * na + c] += 1;
                if i >= 3 {
                    let w = eps[i - 3].0 as usize;
                    self.quad[((w * na + x) * na + y) * na + c] += 1;
                }
            }
        }
        self.trans[prev * (na + 1) + na] += 1;
        self.n_episodes[eps.len().min(MAX_LENGTH)] += 1;
        // Only the activities this person does (bin 0, "none", is filled in
        // by `finish`).
        for &c in &self.touched {
            let c = c as usize;
            let k = self.count[c] as usize;
            self.episodes[c * (MAX_EPISODES + 1) + k.min(MAX_EPISODES)] += 1;
            self.minutes[c * bins + dur_bin(self.total[c] as usize)] += 1;
            self.count[c] = 0;
            self.total[c] = 0;
        }
        self.touched.clear();
        let e = self.sequences.entry(sequence_key(eps)).or_insert((p, 0));
        e.1 += 1;
        self.n_persons += 1;
    }

    pub fn finish(mut self, ds: &Dataset) -> Summary {
        for c in 0..self.n_act {
            let row = &mut self.episodes[c * (MAX_EPISODES + 1)..(c + 1) * (MAX_EPISODES + 1)];
            row[0] = self.n_persons - row[1..].iter().sum::<u32>();
        }
        let n_distinct = self.sequences.len() as u32;
        let mut eps = Vec::new();
        let mut seqs: Vec<(u128, u32, u32)> = self
            .sequences
            .into_iter()
            .map(|(k, (rep, n))| (k, rep, n))
            .collect();
        seqs.sort_unstable_by_key(|&(_, rep, n)| (std::cmp::Reverse(n), rep));
        let len = self.len;
        let mut codes_of = |k: u128, rep: u32, out: &mut Vec<u8>| {
            if !key_codes(k, out) {
                episodes(ds, rep as usize, len, &mut eps);
                out.clear();
                out.extend(eps.iter().map(|e| e.0));
            }
        };
        let top = seqs.len().min(TOP_SEQUENCES);
        let sequences: Vec<Counted> = seqs[..top]
            .iter()
            .map(|&(k, rep, count)| {
                let mut codes = Vec::new();
                codes_of(k, rep, &mut codes);
                Counted { codes, count }
            })
            .collect();
        let na = self.n_act;
        let tail = tail_grid(&seqs[top..], na, |k, rep, out| codes_of(k, rep, out));
        let mut ngrams = top_ngrams(&self.tri, na, 3);
        ngrams.extend(top_ngrams(&self.quad, na, 4));
        Summary {
            n_persons: self.n_persons,
            n_activities: na,
            bin: self.bin as u32,
            bins: self.bins,
            sequences,
            n_distinct,
            tail,
            tail_rows: TAIL_ROWS,
            tail_cols: TAIL_COLS,
            start_dur: self.start_dur,
            episodes: self.episodes,
            minutes: self.minutes,
            n_episodes: self.n_episodes,
            trans: self.trans,
            ngrams,
        }
    }
}

/// Person `p`'s episodes as (code, start, end), merging repeated codes.
fn episodes(ds: &Dataset, p: usize, len: usize, out: &mut Vec<(u8, u16, u16)>) {
    out.clear();
    let segs = ds.person(p);
    for (k, &s) in segs.iter().enumerate() {
        let end = segs.get(k + 1).map_or(len as u16, |&n| seg_start(n));
        match out.last_mut() {
            Some(last) if last.0 == seg_code(s) => last.2 = end,
            _ => out.push((seg_code(s), seg_start(s), end)),
        }
    }
}

/// Exact for up to `EXACT_LEN` episodes (5 bits per code, length in the top
/// bits); longer sequences use two independent hashes.
fn sequence_key(eps: &[(u8, u16, u16)]) -> u128 {
    if eps.len() <= EXACT_LEN {
        // Constant shifts: variable-width u128 shifts are slow here.
        let mut k = 0u128;
        for e in eps {
            k = (k << 5) | e.0 as u128;
        }
        return k | (eps.len() as u128) << 123;
    }
    let (mut h1, mut h2) = (Fx(1), Fx(0x9e37_79b9_7f4a_7c15));
    for e in eps {
        h1.write_u64(e.0 as u64);
        h2.write_u64(e.0 as u64 ^ 0xa5);
    }
    let h = ((h1.finish() as u128) << 64) | h2.finish() as u128;
    (31u128 << 123) | (h & ((1u128 << 123) - 1))
}

/// The codes of an exact sequence key; false for a hashed (long) one.
fn key_codes(k: u128, out: &mut Vec<u8>) -> bool {
    let n = (k >> 123) as usize;
    if n > EXACT_LEN {
        return false;
    }
    out.clear();
    out.extend((0..n).rev().map(|i| ((k >> (5 * i)) & 31) as u8));
    true
}

/// Sums up sequences (in display order) as the `Summary::tail` grid.
fn tail_grid(
    seqs: &[(u128, u32, u32)],
    n_act: usize,
    mut codes_of: impl FnMut(u128, u32, &mut Vec<u8>),
) -> Vec<u32> {
    let (rows, cols) = (TAIL_ROWS as u64, TAIL_COLS as u64);
    let mut out = vec![0u32; TAIL_ROWS * TAIL_COLS * n_act];
    let total: u64 = seqs.iter().map(|s| s.2 as u64).sum();
    if total == 0 {
        return out;
    }
    let mut codes = Vec::new();
    // Persons scaled by `rows`, so row r covers [r * total, (r + 1) * total).
    let mut at = 0u64;
    for &(k, rep, n) in seqs {
        codes_of(k, rep, &mut codes);
        let (a, b) = (at * rows, (at + n as u64) * rows);
        at += n as u64;
        let len = codes.len() as u64;
        for r in a / total..=(b - 1) / total {
            let w = (b.min((r + 1) * total) - a.max(r * total)) as u32;
            for c in 0..cols {
                let code = codes[((2 * c + 1) * len / (2 * cols)) as usize] as usize;
                out[((r * cols + c) as usize) * n_act + code] += w;
            }
        }
    }
    out
}

/// The `TOP_NGRAMS` most frequent n-grams of a dense `[c0][c1]..` table.
fn top_ngrams(table: &[u32], n_act: usize, n: u32) -> Vec<Counted> {
    let mut out: Vec<Counted> = table
        .iter()
        .enumerate()
        .filter(|(_, &c)| c > 0)
        .map(|(mut i, &count)| {
            let mut codes = vec![0u8; n as usize];
            for k in (0..n as usize).rev() {
                codes[k] = (i % n_act) as u8;
                i /= n_act;
            }
            Counted { codes, count }
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.codes.cmp(&b.codes)));
    out.truncate(TOP_NGRAMS);
    out
}

pub fn summarize(ds: &Dataset, rows: &[u32]) -> Summary {
    let mut s = Summarizer::new(ds);
    s.step(ds, rows, rows.len());
    s.finish(ds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::tests::schedules_strategy;
    use crate::testutil::dataset;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    /// Straightforward reference from the (start, code) lists.
    fn naive(len: u32, n_act: usize, sched: &[Vec<(u16, u8)>], rows: &[u32]) -> Summary {
        let bin = (len as usize).div_ceil(MAX_BINS).max(1);
        let bins = (len as usize).div_ceil(bin).max(1);
        // Sequence -> (first person, count).
        let mut seqs: BTreeMap<Vec<u8>, (u32, u32)> = BTreeMap::new();
        let mut grams: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
        let mut start_dur = vec![0; n_act * bins * bins];
        let mut episodes = vec![0; n_act * (MAX_EPISODES + 1)];
        let mut minutes = vec![0; n_act * bins];
        let mut n_episodes = vec![0; MAX_LENGTH + 1];
        let mut trans = vec![0; (n_act + 1) * (n_act + 1)];
        for &p in rows {
            let s = &sched[p as usize];
            // Episodes by minute: a new episode starts where the code changes.
            let code_at = |t: u32| s.iter().rev().find(|x| x.0 as u32 <= t).unwrap().1;
            let mut eps: Vec<(u8, u32, u32)> = Vec::new();
            for t in 0..len {
                let c = code_at(t);
                match eps.last_mut() {
                    Some(e) if e.0 == c => e.2 = t + 1,
                    _ => eps.push((c, t, t + 1)),
                }
            }
            let codes: Vec<u8> = eps.iter().map(|e| e.0).collect();
            seqs.entry(codes.clone()).or_insert((p, 0)).1 += 1;
            for n in 3..=4 {
                for w in codes.windows(n) {
                    *grams.entry(w.to_vec()).or_default() += 1;
                }
            }
            let mut states = vec![n_act];
            states.extend(codes.iter().map(|&c| c as usize));
            states.push(n_act);
            for w in states.windows(2) {
                trans[w[0] * (n_act + 1) + w[1]] += 1;
            }
            n_episodes[eps.len().min(MAX_LENGTH)] += 1;
            for c in 0..n_act {
                let mine: Vec<_> = eps.iter().filter(|e| e.0 as usize == c).collect();
                episodes[c * (MAX_EPISODES + 1) + mine.len().min(MAX_EPISODES)] += 1;
                let total: u32 = mine.iter().map(|e| e.2 - e.1).sum();
                if total > 0 {
                    minutes[c * bins + ((total as usize - 1) / bin).min(bins - 1)] += 1;
                }
                for e in mine {
                    let sb = (e.1 as usize / bin).min(bins - 1);
                    let db = ((e.2 - e.1) as usize - 1) / bin;
                    start_dur[(c * bins + sb) * bins + db.min(bins - 1)] += 1;
                }
            }
        }
        let n_distinct = seqs.len() as u32;
        let top = |m: BTreeMap<Vec<u8>, u32>, k: usize| {
            let mut v: Vec<Counted> = m
                .into_iter()
                .map(|(codes, count)| Counted { codes, count })
                .collect();
            v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.codes.cmp(&b.codes)));
            v.truncate(k);
            v
        };
        let (g3, g4): (BTreeMap<_, _>, BTreeMap<_, _>) =
            grams.into_iter().partition(|(k, _)| k.len() == 3);
        let mut seqs: Vec<_> = seqs.into_iter().collect();
        seqs.sort_by_key(|(_, (rep, n))| (std::cmp::Reverse(*n), *rep));
        // The rarer persons one by one, each covering [i, i + 1) / T of the
        // tail, and the part of that in each grid row.
        let rarer: Vec<&Vec<u8>> = seqs
            .iter()
            .skip(TOP_SEQUENCES)
            .flat_map(|(codes, (_, n))| std::iter::repeat_n(codes, *n as usize))
            .collect();
        let t = rarer.len() as u64;
        let (tr, tc) = (TAIL_ROWS as u64, TAIL_COLS as u64);
        let mut tail = vec![0u32; TAIL_ROWS * TAIL_COLS * n_act];
        for r in 0..tr {
            for (i, codes) in rarer.iter().enumerate() {
                let i = i as u64;
                let lo = (i * tr).max(r * t);
                let hi = ((i + 1) * tr).min((r + 1) * t);
                if hi <= lo {
                    continue;
                }
                for c in 0..tc {
                    // The code at the middle of column c's share of the sequence.
                    let mid = (c as f64 + 0.5) / tc as f64 * codes.len() as f64;
                    let code = codes[mid.floor() as usize] as usize;
                    tail[((r * tc + c) as usize) * n_act + code] += (hi - lo) as u32;
                }
            }
        }
        let sequences = seqs
            .into_iter()
            .take(TOP_SEQUENCES)
            .map(|(codes, (_, count))| Counted { codes, count })
            .collect();
        let mut ngrams = top(g3, TOP_NGRAMS);
        ngrams.extend(top(g4, TOP_NGRAMS));
        Summary {
            n_persons: rows.len() as u32,
            n_activities: n_act,
            bin: bin as u32,
            bins,
            sequences,
            n_distinct,
            tail,
            tail_rows: TAIL_ROWS,
            tail_cols: TAIL_COLS,
            start_dur,
            episodes,
            minutes,
            n_episodes,
            trans,
            ngrams,
        }
    }

    proptest! {
        #[test]
        fn summary_matches_naive(
            (len, n_act, sched) in schedules_strategy(),
            keep in proptest::collection::vec(any::<bool>(), 40),
            budget in prop_oneof![1usize..5, Just(1 << 20)],
        ) {
            let ds = dataset(len, n_act, &sched);
            let rows: Vec<u32> = (0..sched.len() as u32).filter(|&p| keep[p as usize]).collect();
            let mut s = Summarizer::new(&ds);
            while !s.step(&ds, &rows, budget) {}
            prop_assert_eq!(s.finish(&ds), naive(len, n_act, &sched, &rows));
        }
    }

    #[test]
    fn top_sequences_are_cut_like_naive() {
        // Enough distinct sequences (with tied counts) to truncate the list.
        let mut x = 7u64;
        let mut next = |m: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            (x >> 33) % m
        };
        let sched: Vec<Vec<(u16, u8)>> = (0..3000)
            .map(|_| {
                let k = next(6) as u16;
                (0..=k).map(|i| (i * 100, next(5) as u8)).collect()
            })
            .collect();
        let ds = dataset(1440, 5, &sched);
        let rows: Vec<u32> = (0..3000).collect();
        let s = summarize(&ds, &rows);
        assert!(s.n_distinct as usize > TOP_SEQUENCES);
        assert_eq!(s, naive(1440, 5, &sched, &rows));
        // Every tail row covers all the rarer persons in each column.
        let rarer = 3000 - s.sequences.iter().map(|q| q.count).sum::<u32>();
        let cell =
            |r: usize, c: usize| -> u32 { s.tail[(r * TAIL_COLS + c) * 5..][..5].iter().sum() };
        assert!(rarer > 0);
        assert_eq!(cell(0, 0), rarer);
        assert_eq!(cell(TAIL_ROWS - 1, TAIL_COLS - 1), rarer);
    }

    #[test]
    fn long_sequences_are_hashed_and_counted() {
        let long: Vec<(u16, u8)> = (0..40).map(|i| (i * 10, (i % 2) as u8)).collect();
        let mut other = long.clone();
        other[39].1 = 0;
        other[38].1 = 1;
        other[37].1 = 0; // still alternating, but one episode shorter at the end
        let sched = vec![long.clone(), other, long];
        let ds = dataset(1440, 2, &sched);
        let s = summarize(&ds, &[0, 1, 2]);
        assert_eq!(s.n_distinct, 2);
        assert_eq!(s.sequences[0].count, 2);
        assert_eq!(s.sequences[0].codes.len(), 40);
    }
}
