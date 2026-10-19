//! Attribute column builders: the column encodings (narrowest integer
//! type, f32 floats unless f64 is asked for, booleans,
//! dictionary-coded categories in natural order), built row by row so a
//! column never needs more than its final size plus a small dictionary.

use crate::ids::hash;
use crate::table::ColData;
use overact_core::format::{ColumnKind, ColumnMeta, DType};
use overact_core::ColumnData;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};

/// Distinct values above which a text column is dropped while reading.
const MAX_CATEGORIES: usize = 65535;
/// Up to this many categories, values are found by a linear scan.
const SCAN_CATEGORIES: usize = 32;

/// What a column holds, decided from the source type or by CSV inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Int,
    Float,
    Bool,
    Text,
}

/// Infers a CSV column's kind from sample values the way polars does:
/// all integers, else all numbers, else all true/false, else text. A column
/// with no values is text.
pub fn infer<'a>(values: impl Iterator<Item = Option<&'a [u8]>>) -> Kind {
    let (mut int, mut float, mut boolean, mut any) = (true, true, true, false);
    for v in values.flatten() {
        any = true;
        let Ok(s) = std::str::from_utf8(v) else {
            return Kind::Text;
        };
        if int && s.parse::<i64>().is_err() {
            int = false;
        }
        if float && !int && s.parse::<f64>().is_err() {
            float = false;
        }
        if boolean && parse_bool(v).is_none() {
            boolean = false;
        }
        if !int && !float && !boolean {
            return Kind::Text;
        }
    }
    match () {
        _ if !any => Kind::Text,
        _ if int => Kind::Int,
        _ if float => Kind::Float,
        _ if boolean => Kind::Bool,
        _ => Kind::Text,
    }
}

fn parse_bool(v: &[u8]) -> Option<bool> {
    if v.eq_ignore_ascii_case(b"true") {
        Some(true)
    } else if v.eq_ignore_ascii_case(b"false") {
        Some(false)
    } else {
        None
    }
}

/// A value did not fit the column's kind; re-read it as text.
#[derive(Debug)]
pub struct NotThatKind;

#[derive(Clone, Copy, Default)]
struct FastHash;

impl BuildHasher for FastHash {
    type Hasher = BytesHasher;
    fn build_hasher(&self) -> BytesHasher {
        BytesHasher(0)
    }
}

pub struct BytesHasher(u64);

impl Hasher for BytesHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0 = hash(bytes, self.0);
    }
}

enum Ints {
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    F64(Vec<f64>),
}

/// Float values: f32 unless the user asked for f64 (`Config::f64_cols`).
enum Floats {
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl Floats {
    fn new(wide: bool, vals: impl Iterator<Item = f64>) -> Floats {
        if wide {
            Floats::F64(vals.collect())
        } else {
            Floats::F32(vals.map(|x| x as f32).collect())
        }
    }

    #[inline]
    fn push(&mut self, x: f64) {
        match self {
            Floats::F32(v) => v.push(x as f32),
            Floats::F64(v) => v.push(x),
        }
    }
}

enum Data {
    Int {
        v: Ints,
        lo: i64,
        hi: i64,
        any: bool,
    },
    Float {
        v: Floats,
        lo: f64,
        hi: f64,
    },
    Bool(Vec<u8>),
    Text {
        /// Distinct values by code.
        keys: Vec<Box<[u8]>>,
        /// Value -> code, built once there are more than `SCAN_CATEGORIES`.
        dict: HashMap<Box<[u8]>, u16, FastHash>,
        codes: Vec<u16>,
    },
    /// Dropped while reading (too many categories).
    Dropped,
}

pub struct Builder {
    pub name: String,
    data: Data,
    nulls: u64,
    /// Named explicitly by the user: keep even if it looks like an identifier.
    forced: bool,
    /// Store floats as f64 instead of f32.
    wide: bool,
}

/// A finished column.
pub struct Encoded {
    pub meta: ColumnMeta,
    pub data: ColumnData,
}

impl Builder {
    /// `wide`: store floats as f64 (`--f64`).
    pub fn new(name: String, kind: Kind, forced: bool, wide: bool) -> Builder {
        let data = match kind {
            Kind::Int => Data::Int {
                v: Ints::I8(Vec::new()),
                lo: 0,
                hi: 0,
                any: false,
            },
            Kind::Float => Data::Float {
                v: Floats::new(wide, std::iter::empty()),
                lo: f64::INFINITY,
                hi: f64::NEG_INFINITY,
            },
            Kind::Bool => Data::Bool(Vec::new()),
            Kind::Text => Data::Text {
                keys: Vec::new(),
                dict: HashMap::with_hasher(FastHash),
                codes: Vec::new(),
            },
        };
        Builder {
            name,
            data,
            nulls: 0,
            forced,
            wide,
        }
    }

    /// Appends row `i` of `col`. `warnings` gets a note if the column is dropped.
    #[inline]
    pub fn push(
        &mut self,
        col: &ColData,
        i: usize,
        buf: &mut Vec<u8>,
        warnings: &mut Vec<String>,
    ) -> Result<(), NotThatKind> {
        match &mut self.data {
            Data::Dropped => {}
            Data::Int { .. } => match col {
                ColData::Int(v, ok) => self.push_int(ok[i].then_some(v[i])),
                ColData::Text(t) => match t.get(i) {
                    None => self.push_int(None),
                    Some(b) => match std::str::from_utf8(b)
                        .ok()
                        .and_then(|s| s.parse::<i64>().ok())
                    {
                        Some(x) => self.push_int(Some(x)),
                        None => {
                            // A float in an integer column: widen to float.
                            let x = std::str::from_utf8(b)
                                .ok()
                                .and_then(|s| s.parse::<f64>().ok())
                                .ok_or(NotThatKind)?;
                            self.widen_to_float();
                            self.push_float(Some(x));
                        }
                    },
                },
                _ => return Err(NotThatKind),
            },
            Data::Float { .. } => {
                let x = col.num(i).map_err(|_| NotThatKind)?;
                self.push_float(x)
            }
            Data::Bool(v) => {
                let x = match col {
                    ColData::Bool(b) => b[i],
                    ColData::Text(t) => match t.get(i) {
                        None => 255,
                        Some(b) => parse_bool(b).ok_or(NotThatKind)? as u8,
                    },
                    _ => return Err(NotThatKind),
                };
                v.push(x);
                self.nulls += (x == 255) as u64;
            }
            Data::Text { keys, dict, codes } => match col.key(i, buf) {
                None => {
                    codes.push(u16::MAX);
                    self.nulls += 1;
                }
                Some(k) => {
                    // Few categories (the usual case): a linear scan beats hashing.
                    let found = if keys.len() <= SCAN_CATEGORIES {
                        keys.iter().position(|x| **x == *k).map(|c| c as u16)
                    } else {
                        dict.get(k).copied()
                    };
                    let code = match found {
                        Some(c) => c,
                        None => {
                            if keys.len() >= MAX_CATEGORIES {
                                warnings.push(self.too_many_message());
                                self.data = Data::Dropped;
                                return Ok(());
                            }
                            let c = keys.len() as u16;
                            keys.push(k.into());
                            if keys.len() > SCAN_CATEGORIES {
                                if dict.is_empty() {
                                    dict.extend(
                                        keys.iter().enumerate().map(|(c, x)| (x.clone(), c as u16)),
                                    );
                                } else {
                                    dict.insert(k.into(), c);
                                }
                            }
                            c
                        }
                    };
                    codes.push(code);
                }
            },
        }
        Ok(())
    }

    fn too_many_message(&self) -> String {
        if self.forced {
            format!(
                "column {:?}: more than {MAX_CATEGORIES} distinct values is too many for a category; skipped",
                self.name
            )
        } else {
            format!(
                "column {:?}: more than {MAX_CATEGORIES} distinct values looks like an identifier; skipped",
                self.name
            )
        }
    }

    #[inline]
    fn push_int(&mut self, x: Option<i64>) {
        let Data::Int { v, lo, hi, any } = &mut self.data else {
            unreachable!()
        };
        let Some(x) = x else {
            self.nulls += 1;
            match v {
                Ints::I8(v) => v.push(i8::MIN),
                Ints::I16(v) => v.push(i16::MIN),
                Ints::I32(v) => v.push(i32::MIN),
                Ints::F64(v) => v.push(f64::NAN),
            }
            return;
        };
        if *any {
            *lo = (*lo).min(x);
            *hi = (*hi).max(x);
        } else {
            (*lo, *hi, *any) = (x, x, true);
        }
        // Widen when the value does not fit (the type minimum is the null).
        let fits = |min: i64, max: i64| min < *lo && *hi <= max;
        loop {
            match v {
                Ints::I8(a) if fits(i8::MIN as i64, i8::MAX as i64) => return a.push(x as i8),
                Ints::I16(a) if fits(i16::MIN as i64, i16::MAX as i64) => return a.push(x as i16),
                Ints::I32(a) if fits(i32::MIN as i64, i32::MAX as i64) => return a.push(x as i32),
                Ints::F64(a) => return a.push(x as f64),
                Ints::I8(a) => {
                    *v = Ints::I16(
                        a.iter()
                            .map(|&y| if y == i8::MIN { i16::MIN } else { y as i16 })
                            .collect(),
                    )
                }
                Ints::I16(a) => {
                    *v = Ints::I32(
                        a.iter()
                            .map(|&y| if y == i16::MIN { i32::MIN } else { y as i32 })
                            .collect(),
                    )
                }
                Ints::I32(a) => {
                    *v = Ints::F64(
                        a.iter()
                            .map(|&y| if y == i32::MIN { f64::NAN } else { y as f64 })
                            .collect(),
                    )
                }
            }
        }
    }

    fn widen_to_float(&mut self) {
        let Data::Int { v, lo, hi, any } = &self.data else {
            return;
        };
        // Integers are exact in f64, so going through it rounds only once.
        let wide = self.wide;
        let vals = match v {
            Ints::I8(a) => Floats::new(
                wide,
                a.iter()
                    .map(|&y| if y == i8::MIN { f64::NAN } else { y as f64 }),
            ),
            Ints::I16(a) => Floats::new(
                wide,
                a.iter()
                    .map(|&y| if y == i16::MIN { f64::NAN } else { y as f64 }),
            ),
            Ints::I32(a) => Floats::new(
                wide,
                a.iter()
                    .map(|&y| if y == i32::MIN { f64::NAN } else { y as f64 }),
            ),
            Ints::F64(a) => Floats::new(wide, a.iter().copied()),
        };
        let (lo, hi) = if *any {
            (*lo as f64, *hi as f64)
        } else {
            (f64::INFINITY, f64::NEG_INFINITY)
        };
        self.data = Data::Float { v: vals, lo, hi };
    }

    #[inline]
    fn push_float(&mut self, x: Option<f64>) {
        let Data::Float { v, lo, hi } = &mut self.data else {
            unreachable!()
        };
        match x.filter(|x| !x.is_nan()) {
            None => {
                self.nulls += 1;
                v.push(f64::NAN);
            }
            Some(x) => {
                *lo = lo.min(x);
                *hi = hi.max(x);
                v.push(x);
            }
        }
    }

    /// Finishes the column with `extra` trailing missing values (persons with
    /// activities but no attributes). `n_rows` is the number of attribute rows.
    pub fn finish(
        self,
        extra: usize,
        n_rows: usize,
        warnings: &mut Vec<String>,
    ) -> Option<Encoded> {
        let nulls = self.nulls + extra as u64;
        let meta = |kind,
                    dtype,
                    null_value: Option<f64>,
                    min: Option<f64>,
                    max: Option<f64>,
                    categories| ColumnMeta {
            name: self.name.clone(),
            kind,
            dtype,
            byte_offset: 0,
            byte_length: 0,
            null_value,
            null_count: Some(nulls),
            min,
            max,
            categories,
        };
        fn pad<T: Copy>(mut v: Vec<T>, extra: usize, null: T) -> Vec<T> {
            v.extend(std::iter::repeat_n(null, extra));
            v
        }
        Some(match self.data {
            Data::Dropped => return None,
            Data::Int { v, lo, hi, any } => {
                let (min, max) = if any {
                    (Some(lo as f64), Some(hi as f64))
                } else {
                    (None, None)
                };
                match v {
                    Ints::I8(a) => Encoded {
                        meta: meta(
                            ColumnKind::Numeric,
                            DType::I8,
                            Some(i8::MIN as f64),
                            min,
                            max,
                            None,
                        ),
                        data: ColumnData::I8(pad(a, extra, i8::MIN)),
                    },
                    Ints::I16(a) => Encoded {
                        meta: meta(
                            ColumnKind::Numeric,
                            DType::I16,
                            Some(i16::MIN as f64),
                            min,
                            max,
                            None,
                        ),
                        data: ColumnData::I16(pad(a, extra, i16::MIN)),
                    },
                    Ints::I32(a) => Encoded {
                        meta: meta(
                            ColumnKind::Numeric,
                            DType::I32,
                            Some(i32::MIN as f64),
                            min,
                            max,
                            None,
                        ),
                        data: ColumnData::I32(pad(a, extra, i32::MIN)),
                    },
                    Ints::F64(a) => {
                        warnings.push(format!(
                            "column {:?}: integers exceed i32, stored as f64",
                            self.name
                        ));
                        Encoded {
                            meta: meta(ColumnKind::Numeric, DType::F64, None, min, max, None),
                            data: ColumnData::F64(pad(a, extra, f64::NAN)),
                        }
                    }
                }
            }
            Data::Float { v, lo, hi } => {
                let any = lo <= hi;
                match v {
                    Floats::F32(v) => Encoded {
                        meta: meta(
                            ColumnKind::Numeric,
                            DType::F32,
                            None,
                            any.then_some(lo as f32 as f64),
                            any.then_some(hi as f32 as f64),
                            None,
                        ),
                        data: ColumnData::F32(pad(v, extra, f32::NAN)),
                    },
                    Floats::F64(v) => Encoded {
                        meta: meta(
                            ColumnKind::Numeric,
                            DType::F64,
                            None,
                            any.then_some(lo),
                            any.then_some(hi),
                            None,
                        ),
                        data: ColumnData::F64(pad(v, extra, f64::NAN)),
                    },
                }
            }
            Data::Bool(v) => {
                let has = |b| v.contains(&b);
                let (min, max) = match (has(0), has(1)) {
                    (false, false) => (None, None),
                    (f, t) => (
                        Some(if f { 0.0 } else { 1.0 }),
                        Some(if t { 1.0 } else { 0.0 }),
                    ),
                };
                Encoded {
                    meta: meta(ColumnKind::Boolean, DType::U8, Some(255.0), min, max, None),
                    data: ColumnData::U8(pad(v, extra, 255)),
                }
            }
            Data::Text { keys, codes, .. } => {
                let n_cats = keys.len();
                if !self.forced && n_cats > 1000.max(n_rows / 2) {
                    warnings.push(format!(
                        "column {:?}: {n_cats} distinct values looks like an identifier; skipped",
                        self.name
                    ));
                    return None;
                }
                let mut values: Vec<(String, u16)> = keys
                    .iter()
                    .enumerate()
                    .map(|(c, k)| (String::from_utf8_lossy(k).into_owned(), c as u16))
                    .collect();
                values.sort_by(|a, b| a.0.cmp(&b.0));
                let order =
                    order_categories(&values.iter().map(|v| v.0.as_str()).collect::<Vec<_>>());
                let mut lut = vec![0u16; n_cats];
                let cats: Vec<String> = order
                    .iter()
                    .enumerate()
                    .map(|(new, &old)| {
                        lut[values[old].1 as usize] = new as u16;
                        values[old].0.clone()
                    })
                    .collect();
                let (min, max) = if n_cats > 0 {
                    (Some(0.0), Some((n_cats - 1) as f64))
                } else {
                    (None, None)
                };
                if n_cats <= 255 {
                    let v = codes
                        .iter()
                        .map(|&c| {
                            if c == u16::MAX {
                                255
                            } else {
                                lut[c as usize] as u8
                            }
                        })
                        .collect();
                    Encoded {
                        meta: meta(
                            ColumnKind::Categorical,
                            DType::U8,
                            Some(255.0),
                            min,
                            max,
                            Some(cats),
                        ),
                        data: ColumnData::U8(pad(v, extra, 255)),
                    }
                } else {
                    let v = codes
                        .iter()
                        .map(|&c| if c == u16::MAX { c } else { lut[c as usize] })
                        .collect();
                    Encoded {
                        meta: meta(
                            ColumnKind::Categorical,
                            DType::U16,
                            Some(65535.0),
                            min,
                            max,
                            Some(cats),
                        ),
                        data: ColumnData::U16(pad(v, extra, u16::MAX)),
                    }
                }
            }
        })
    }
}

const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// Display order of category values (indices into `values`): weekday names
/// in week order, otherwise natural order ("2" < "10"), case-insensitive.
pub fn order_categories(values: &[&str]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..values.len()).collect();
    let low: Vec<String> = values.iter().map(|v| v.to_lowercase()).collect();
    let full = low.iter().all(|v| WEEKDAYS.contains(&v.as_str()));
    let short = low.iter().all(|v| WEEKDAYS.iter().any(|d| &d[..3] == v));
    if !values.is_empty() && (full || short) {
        let day = |v: &str| {
            WEEKDAYS
                .iter()
                .position(|d| d.get(..v.len()) == Some(v))
                .unwrap()
        };
        idx.sort_by_key(|&i| day(&low[i]));
        return idx;
    }
    idx.sort_by(|&a, &b| natural_cmp(&low[a], &low[b]));
    idx
}

/// Compares strings split into alternating text and digit runs; digit runs
/// compare as integers.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::Equal;
    let (mut x, mut y) = (tokens(a), tokens(b));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some((p, dp)), Some((q, dq))) => {
                let o = if dp && dq {
                    let (p, q) = (p.trim_start_matches('0'), q.trim_start_matches('0'));
                    p.len().cmp(&q.len()).then_with(|| p.cmp(q))
                } else {
                    p.cmp(q)
                };
                if o != Equal {
                    return o;
                }
            }
        }
    }
}

/// Splits like Python's `re.split(r"(\d+)", s)`: text, digits, text, …
/// (always starting and ending with a possibly empty text token).
fn tokens(s: &str) -> impl Iterator<Item = (&str, bool)> {
    let b = s.as_bytes();
    let mut pos = 0;
    let mut digits = false;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let start = pos;
        while pos < b.len() && b[pos].is_ascii_digit() == digits {
            pos += 1;
        }
        let tok = (&s[start..pos], digits);
        if pos == b.len() && !digits {
            done = true;
        }
        digits = !digits;
        Some(tok)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(v: &[&str]) -> Vec<String> {
        order_categories(v)
            .into_iter()
            .map(|i| v[i].to_string())
            .collect()
    }

    #[test]
    fn natural_and_weekday_order() {
        assert_eq!(order(&["10", "2", "1"]), ["1", "2", "10"]);
        assert_eq!(order(&["b2", "a10", "a9", "B1"]), ["a9", "a10", "B1", "b2"]);
        assert_eq!(
            order(&["sunday", "monday", "friday"]),
            ["monday", "friday", "sunday"]
        );
        assert_eq!(order(&["Sun", "Mon"]), ["Mon", "Sun"]);
    }

    #[test]
    fn infers_like_polars() {
        let k = |v: &[&str]| infer(v.iter().map(|s| (!s.is_empty()).then_some(s.as_bytes())));
        assert_eq!(k(&["1", "", "-3"]), Kind::Int);
        assert_eq!(k(&["1", "2.5"]), Kind::Float);
        assert_eq!(k(&["true", "False"]), Kind::Bool);
        assert_eq!(k(&["yes", "1"]), Kind::Text);
        assert_eq!(k(&["", ""]), Kind::Text);
    }

    #[test]
    fn ints_narrow_and_widen() {
        let mut b = Builder::new("x".into(), Kind::Int, false, false);
        let col = ColData::Int(vec![5, 0, 300, -128], vec![true, false, true, true]);
        let mut w = vec![];
        for i in 0..4 {
            b.push(&col, i, &mut vec![], &mut w).unwrap();
        }
        let e = b.finish(1, 4, &mut w).unwrap();
        assert_eq!(e.meta.dtype, DType::I16);
        let ColumnData::I16(v) = e.data else { panic!() };
        assert_eq!(v, vec![5, i16::MIN, 300, -128, i16::MIN]);
        assert_eq!(
            (e.meta.min, e.meta.max, e.meta.null_count),
            (Some(-128.0), Some(300.0), Some(2))
        );
    }
}
