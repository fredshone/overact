//! Filter conditions over attribute and virtual columns.

use crate::column::Column;
use crate::features::Feature;
use serde::{Deserialize, Serialize};

/// A reference to a column: an attribute in the file or a derived feature.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColRef {
    Attr { column: usize },
    Virtual(Feature),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Test {
    /// `min <= value <= max` (either bound optional). Missing values fail.
    Range { min: Option<f64>, max: Option<f64> },
    /// Value is one of `values` (category indices; 0/1 for booleans).
    /// `null` makes missing values pass too.
    In {
        values: Vec<u32>,
        #[serde(default)]
        null: bool,
    },
    /// Value is missing.
    IsNull,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub col: ColRef,
    #[serde(flatten)]
    pub test: Test,
    #[serde(default)]
    pub negate: bool,
}

/// A condition bound to its column, ready to evaluate.
///
/// Common cases get a precompiled predicate so the scan loops are tight and
/// free of per-element type dispatch; everything else uses `test`.
pub struct Bound<'a> {
    col: &'a Column,
    test: Test,
    /// For `In`: membership by category index.
    set: Vec<bool>,
    negate: bool,
    fast: Fast<'a>,
}

enum Fast<'a> {
    /// Lookup table over every u8 value (including the null sentinel).
    U8(&'a [u8], Box<[bool; 256]>),
    /// Lookup table over every u16 value.
    U16(&'a [u16], Vec<bool>),
    /// Inclusive integer range; values equal to `null` are missing.
    I8(&'a [i8], i64, i64, i64),
    I16(&'a [i16], i64, i64, i64),
    I32(&'a [i32], i64, i64, i64),
    F32(&'a [f32], f64, f64),
    None,
}

/// Runs `$body` with `$pass: Fn(usize) -> bool` bound to the condition's
/// tight per-type predicate (negation applied).
macro_rules! with_pass {
    ($b:expr, $pass:ident => $body:expr) => {{
        let b = $b;
        let neg = b.negate;
        match &b.fast {
            Fast::U8(d, lut) => {
                let $pass = |i: usize| lut[d[i] as usize];
                $body
            }
            Fast::U16(d, lut) => {
                let $pass = |i: usize| lut[d[i] as usize];
                $body
            }
            Fast::I8(d, lo, hi, null) => {
                let $pass = |i: usize| {
                    let v = d[i] as i64;
                    (v != *null && v >= *lo && v <= *hi) != neg
                };
                $body
            }
            Fast::I16(d, lo, hi, null) => {
                let $pass = |i: usize| {
                    let v = d[i] as i64;
                    (v != *null && v >= *lo && v <= *hi) != neg
                };
                $body
            }
            Fast::I32(d, lo, hi, null) => {
                let $pass = |i: usize| {
                    let v = d[i] as i64;
                    (v != *null && v >= *lo && v <= *hi) != neg
                };
                $body
            }
            // NaN (missing) fails both comparisons.
            Fast::F32(d, lo, hi) => {
                let $pass = |i: usize| {
                    let v = d[i] as f64;
                    (v >= *lo && v <= *hi) != neg
                };
                $body
            }
            Fast::None => {
                let $pass = |i: usize| b.test(i);
                $body
            }
        }
    }};
}

impl<'a> Bound<'a> {
    pub fn new(cond: &Condition, col: &'a Column) -> Bound<'a> {
        let mut set = Vec::new();
        if let Test::In { values, .. } = &cond.test {
            let n = values.iter().copied().max().map_or(0, |m| m as usize + 1);
            set = vec![false; n];
            for &v in values {
                set[v as usize] = true;
            }
        }
        let mut b = Bound {
            col,
            test: cond.test.clone(),
            set,
            negate: cond.negate,
            fast: Fast::None,
        };
        b.fast = b.compile();
        b
    }

    fn compile(&self) -> Fast<'a> {
        use crate::column::ColumnData as D;
        let col = self.col;
        match (&col.data, &self.test) {
            (D::U8(d), _) => {
                let mut lut = Box::new([false; 256]);
                for (v, slot) in lut.iter_mut().enumerate() {
                    *slot = self.test_value(col_value_u8(col, v as u8));
                }
                Fast::U8(d, lut)
            }
            (D::U16(d), _) => {
                let lut = (0..=u16::MAX)
                    .map(|v| self.test_value(col_value_u16(col, v)))
                    .collect();
                Fast::U16(d, lut)
            }
            (D::I8(d), Test::Range { min, max }) => {
                let (lo, hi) = int_bounds(*min, *max);
                Fast::I8(d, lo, hi, i8::MIN as i64)
            }
            (D::I16(d), Test::Range { min, max }) => {
                let (lo, hi) = int_bounds(*min, *max);
                Fast::I16(d, lo, hi, i16::MIN as i64)
            }
            (D::I32(d), Test::Range { min, max }) => {
                let (lo, hi) = int_bounds(*min, *max);
                Fast::I32(d, lo, hi, i32::MIN as i64)
            }
            (D::F32(d), Test::Range { min, max }) => Fast::F32(
                d,
                min.unwrap_or(f64::NEG_INFINITY),
                max.unwrap_or(f64::INFINITY),
            ),
            _ => Fast::None,
        }
    }

    /// The result (after negation) for a value.
    #[inline]
    fn test_value(&self, v: Option<f64>) -> bool {
        let r = match (&self.test, v) {
            (Test::IsNull, v) => v.is_none(),
            (Test::Range { .. }, None) => false,
            (Test::Range { min, max }, Some(v)) => {
                min.is_none_or(|m| v >= m) && max.is_none_or(|m| v <= m)
            }
            (Test::In { null, .. }, None) => *null,
            (Test::In { .. }, Some(v)) => {
                v >= 0.0 && v.fract() == 0.0 && self.set.get(v as usize).copied().unwrap_or(false)
            }
        };
        r != self.negate
    }

    #[inline]
    pub fn test(&self, i: usize) -> bool {
        self.test_value(self.col.get(i))
    }

    /// Fraction of a strided sample of `n` persons that pass (for ordering).
    pub fn selectivity(&self, n: usize) -> f64 {
        if n == 0 {
            return 0.0;
        }
        let k = n.min(4096);
        let pass = (0..k).filter(|&j| self.test(j * n / k)).count();
        pass as f64 / k as f64
    }

    /// Writes each index from `idx` that passes into `out` (which must be at
    /// least as long as `idx`) and returns how many passed. Branchless: every
    /// candidate is written and the cursor only advances on a pass, which is
    /// much faster than branching when the pass rate is near 50%.
    #[inline(always)]
    fn compact(&self, idx: impl Iterator<Item = usize>, out: &mut [u32]) -> usize {
        let mut n = 0;
        with_pass!(self, pass => for i in idx {
            out[n] = i as u32;
            n += pass(i) as usize;
        });
        n
    }

    /// `compact` of `rows[a..b]` into `rows[w..]` in place (`w <= a`, so
    /// every element is read before its slot can be overwritten).
    #[inline(always)]
    fn compact_in_place(&self, rows: &mut [u32], a: usize, b: usize, mut w: usize) -> usize {
        debug_assert!(w <= a);
        with_pass!(self, pass => for j in a..b {
            let p = rows[j];
            rows[w] = p;
            w += pass(p as usize) as usize;
        });
        w
    }
}

fn col_value_u8(col: &Column, v: u8) -> Option<f64> {
    (col.meta.null_value != Some(v as f64)).then_some(v as f64)
}

fn col_value_u16(col: &Column, v: u16) -> Option<f64> {
    (col.meta.null_value != Some(v as f64)).then_some(v as f64)
}

/// Inclusive integer bounds equivalent to real bounds `[min, max]`.
fn int_bounds(min: Option<f64>, max: Option<f64>) -> (i64, i64) {
    let lo = min.map_or(i64::MIN, |m| {
        if m.is_nan() {
            i64::MAX
        } else {
            m.ceil().max(-1e18) as i64
        }
    });
    let hi = max.map_or(i64::MAX, |m| {
        if m.is_nan() {
            i64::MIN
        } else {
            m.floor().min(1e18) as i64
        }
    });
    (lo, hi)
}

/// Below this many elements the parallel paths are not worth their overhead.
#[cfg(feature = "parallel")]
const PAR_MIN: usize = 1 << 17;
#[cfg(feature = "parallel")]
const PAR_CHUNK: usize = 1 << 16;

/// Appends persons in `[a, b)` that pass `cond` to `out`.
pub fn scan(cond: &Bound, a: usize, b: usize, out: &mut Vec<u32>) {
    #[cfg(feature = "parallel")]
    if b - a >= PAR_MIN && rayon::current_num_threads() > 1 {
        use rayon::prelude::*;
        let parts: Vec<Vec<u32>> = (a..b)
            .step_by(PAR_CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|s| {
                let e = (s + PAR_CHUNK).min(b);
                let mut v = vec![0u32; e - s];
                let n = cond.compact(s..e, &mut v);
                v.truncate(n);
                v
            })
            .collect();
        out.reserve(parts.iter().map(Vec::len).sum());
        for p in parts {
            out.extend_from_slice(&p);
        }
        return;
    }
    let base = out.len();
    out.resize(base + (b - a), 0);
    let n = cond.compact(a..b, &mut out[base..]);
    out.truncate(base + n);
}

/// Keeps the elements of `rows[a..b]` that pass, compacting into `rows[w..]`
/// (`w <= a`). Returns the new write position.
pub fn refine(cond: &Bound, rows: &mut [u32], a: usize, b: usize, w: usize) -> usize {
    #[cfg(feature = "parallel")]
    if b - a >= PAR_MIN && rayon::current_num_threads() > 1 {
        use rayon::prelude::*;
        let parts: Vec<Vec<u32>> = rows[a..b]
            .par_chunks(PAR_CHUNK)
            .map(|c| {
                let mut v = vec![0u32; c.len()];
                let n = cond.compact(c.iter().map(|&p| p as usize), &mut v);
                v.truncate(n);
                v
            })
            .collect();
        let mut w = w;
        for p in parts {
            rows[w..w + p.len()].copy_from_slice(&p);
            w += p.len();
        }
        return w;
    }
    cond.compact_in_place(rows, a, b, w)
}
