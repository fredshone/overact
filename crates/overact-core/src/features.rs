//! Derived schedule features ("virtual columns").
//!
//! A feature is computed once per dataset, cached, and then filters and sorts
//! exactly like an attribute column. Times are minutes from the axis start.

use crate::column::{Column, ColumnData};
use crate::dataset::Dataset;
use crate::format::{seg_code, seg_start, ColumnKind, ColumnMeta, DType, Result};
use serde::{Deserialize, Serialize};

/// What a `Feature::Measure` reports about the episodes it selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// Number of episodes (with `nth`: 1 if that episode exists, else 0).
    Count,
    /// Start time of the episode (of all: the first one's).
    Start,
    /// Duration of the episode (of all: the total).
    Duration,
    /// End time of the episode (of all: the last one's).
    End,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "feature", rename_all = "snake_case")]
pub enum Feature {
    /// A measure of a person's episodes (segments). The episodes are those of
    /// `activity` (any activity if None; every other activity with `not`),
    /// and of those the `nth` (1 = first, -1 = last), or all if `nth` is 0.
    /// A missing episode gives null, except that a count is 0 and the total
    /// duration of no episodes is 0.
    Measure {
        measure: Measure,
        #[serde(default)]
        activity: Option<u8>,
        #[serde(default)]
        not: bool,
        #[serde(default)]
        nth: i16,
    },
    /// Number of distinct activities in the day.
    Distinct,
    /// Doing the activity at `minute`.
    DoingAt { activity: u8, minute: u16 },
    /// Doing the activity during `[start, end)`: at any minute, or (with
    /// `all`) at every minute.
    DoingDuring {
        activity: u8,
        start: u16,
        end: u16,
        all: bool,
    },
}

impl Feature {
    pub fn kind(&self) -> ColumnKind {
        match self {
            Feature::DoingAt { .. } | Feature::DoingDuring { .. } => ColumnKind::Boolean,
            _ => ColumnKind::Numeric,
        }
    }

    fn dtype(&self) -> DType {
        match self.kind() {
            ColumnKind::Boolean => DType::U8,
            _ => DType::I16,
        }
    }

    /// A stable name, used as the column name of the computed column.
    pub fn key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Allocates the output column for this feature.
    pub fn alloc(&self, n: usize) -> Result<Column> {
        let dtype = self.dtype();
        let meta = ColumnMeta {
            name: self.key(),
            kind: self.kind(),
            dtype,
            byte_offset: 0,
            byte_length: (n * dtype.size()) as u64,
            null_value: Some(if dtype == DType::U8 { 255.0 } else { -32768.0 }),
            null_count: None,
            min: None,
            max: None,
            categories: None,
        };
        Column::new(meta, ColumnData::zeroed(dtype, n)?)
    }

    /// Computes persons `[a, b)` into `col`.
    pub fn fill(&self, ds: &Dataset, col: &mut Column, a: usize, b: usize) {
        let len = ds.axis_len() as u16;
        #[cfg(feature = "parallel")]
        if b - a >= 1 << 16 && rayon::current_num_threads() > 1 {
            use rayon::prelude::*;
            const C: usize = 1 << 14;
            match &mut col.data {
                ColumnData::I16(out) => {
                    out[a..b].par_chunks_mut(C).enumerate().for_each(|(i, o)| {
                        for (k, v) in o.iter_mut().enumerate() {
                            *v = self.numeric(ds.person(a + i * C + k), len);
                        }
                    })
                }
                ColumnData::U8(out) => {
                    out[a..b].par_chunks_mut(C).enumerate().for_each(|(i, o)| {
                        for (k, v) in o.iter_mut().enumerate() {
                            *v = self.boolean(ds.person(a + i * C + k), len) as u8;
                        }
                    })
                }
                _ => unreachable!("features are i16 or u8"),
            }
            return;
        }
        match &mut col.data {
            ColumnData::I16(out) => {
                for (p, o) in (a..b).zip(&mut out[a..b]) {
                    *o = self.numeric(ds.person(p), len);
                }
            }
            ColumnData::U8(out) => {
                for (p, o) in (a..b).zip(&mut out[a..b]) {
                    *o = self.boolean(ds.person(p), len) as u8;
                }
            }
            _ => unreachable!("features are i16 or u8"),
        }
    }

    /// Sets min/max/nullCount once every person has been filled.
    pub fn finish(&self, col: &mut Column) {
        let (mut lo, mut hi, mut nulls) = (f64::INFINITY, f64::NEG_INFINITY, 0u64);
        for i in 0..col.len() {
            match col.get(i) {
                Some(v) => {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                None => nulls += 1,
            }
        }
        col.meta.null_count = Some(nulls);
        if lo <= hi {
            col.meta.min = Some(lo);
            col.meta.max = Some(hi);
        }
    }

    /// Total minutes in `activity` (a common measure).
    pub fn minutes(activity: u8) -> Feature {
        Feature::Measure {
            measure: Measure::Duration,
            activity: Some(activity),
            not: false,
            nth: 0,
        }
    }

    /// Start of the first episode of `activity` (null if never done).
    pub fn first_start(activity: u8) -> Feature {
        Feature::Measure {
            measure: Measure::Start,
            activity: Some(activity),
            not: false,
            nth: 1,
        }
    }

    fn numeric(&self, segs: &[u16], len: u16) -> i16 {
        match *self {
            Feature::Measure {
                measure: m,
                activity,
                not,
                nth,
            } => measure(segs, len, m, activity, not, nth),
            Feature::Distinct => {
                let mut seen = 0u32;
                for &s in segs {
                    seen |= 1 << seg_code(s);
                }
                seen.count_ones() as i16
            }
            _ => unreachable!(),
        }
    }

    fn boolean(&self, segs: &[u16], len: u16) -> bool {
        let end = |k: usize| segs.get(k + 1).map_or(len, |&s| seg_start(s));
        match *self {
            Feature::DoingAt { activity, minute } => {
                // Last segment starting at or before `minute`.
                let k = segs.partition_point(|&s| seg_start(s) <= minute);
                k > 0 && minute < len && seg_code(segs[k - 1]) == activity
            }
            Feature::DoingDuring {
                activity,
                start,
                end: stop,
                all,
            } => {
                let stop = stop.min(len);
                if start >= stop {
                    return false;
                }
                let mut covered = 0u16;
                for (k, &s) in segs.iter().enumerate() {
                    let (a, b) = (seg_start(s).max(start), end(k).min(stop));
                    if a < b && seg_code(s) == activity {
                        if !all {
                            return true;
                        }
                        covered += b - a;
                    }
                }
                all && covered == stop - start
            }
            _ => unreachable!(),
        }
    }
}

/// See `Feature::Measure`. Start and end of all episodes mean the first
/// start and the last end.
fn measure(segs: &[u16], len: u16, m: Measure, activity: Option<u8>, not: bool, nth: i16) -> i16 {
    let end = |k: usize| segs.get(k + 1).map_or(len, |&s| seg_start(s));
    let mut eps = segs
        .iter()
        .enumerate()
        .map(|(k, &s)| (seg_start(s), end(k), seg_code(s)))
        .filter(|t| activity.is_none_or(|a| (t.2 == a) != not));
    let nth = match (m, nth) {
        (Measure::Start, 0) => 1,
        (Measure::End, 0) => -1,
        _ => nth,
    };
    if nth == 0 {
        return match m {
            Measure::Count => eps.count() as i16,
            _ => eps.map(|t| (t.1 - t.0) as i16).sum(),
        };
    }
    let ep = if nth > 0 {
        eps.nth(nth as usize - 1)
    } else {
        eps.rev().nth((-(nth as i32)) as usize - 1)
    };
    match (m, ep) {
        (Measure::Count, ep) => ep.is_some() as i16,
        (_, None) => i16::MIN,
        (Measure::Start, Some(t)) => t.0 as i16,
        (Measure::End, Some(t)) => t.1 as i16,
        (Measure::Duration, Some(t)) => (t.1 - t.0) as i16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::dataset;

    #[test]
    fn features_match_minute_grid() {
        // 0: a0 0-10, a1 10-30, a0 30-60, a2 60-100
        let ds = dataset(
            100,
            3,
            &[vec![(0, 0), (10, 1), (30, 0), (60, 2)], vec![(0, 2)]],
        );
        let get = |f: Feature, p: usize| {
            let mut c = f.alloc(2).unwrap();
            f.fill(&ds, &mut c, 0, 2);
            c.get(p)
        };
        let m = |measure, activity, not, nth| Feature::Measure {
            measure,
            activity,
            not,
            nth,
        };
        use Measure::*;
        assert_eq!(get(Feature::minutes(0), 0), Some(40.0));
        assert_eq!(get(Feature::first_start(1), 0), Some(10.0));
        assert_eq!(get(Feature::first_start(1), 1), None);
        assert_eq!(get(m(End, Some(0), false, -1), 0), Some(60.0));
        assert_eq!(get(m(End, Some(2), false, -1), 1), Some(100.0));
        assert_eq!(get(m(Count, Some(0), false, 0), 0), Some(2.0));
        assert_eq!(get(m(Count, None, false, 0), 0), Some(4.0));
        // End of the first episode, duration of the second.
        assert_eq!(get(m(End, None, false, 1), 0), Some(10.0));
        assert_eq!(get(m(Duration, None, false, 2), 0), Some(20.0));
        assert_eq!(get(m(Duration, None, false, 2), 1), None);
        // Time not in activity 0, and its episodes.
        assert_eq!(get(m(Duration, Some(0), true, 0), 0), Some(60.0));
        assert_eq!(get(m(Count, Some(0), true, 0), 0), Some(2.0));
        // Last and second-to-last episodes of activity 0.
        assert_eq!(get(m(Start, Some(0), false, -1), 0), Some(30.0));
        assert_eq!(get(m(Start, Some(0), false, -2), 0), Some(0.0));
        assert_eq!(get(m(Start, Some(0), false, -3), 0), None);
        // Start/end of "all" mean the first start and the last end.
        assert_eq!(get(m(Start, Some(1), false, 0), 0), Some(10.0));
        assert_eq!(get(m(End, Some(0), false, 0), 0), Some(60.0));
        assert_eq!(get(m(Count, None, false, 5), 0), Some(0.0));
        assert_eq!(get(m(Count, None, false, 4), 0), Some(1.0));
        assert_eq!(get(Feature::Distinct, 0), Some(3.0));
        assert_eq!(
            get(
                Feature::DoingAt {
                    activity: 1,
                    minute: 29
                },
                0
            ),
            Some(1.0)
        );
        assert_eq!(
            get(
                Feature::DoingAt {
                    activity: 1,
                    minute: 30
                },
                0
            ),
            Some(0.0)
        );
        let during = |s, e, all| Feature::DoingDuring {
            activity: 0,
            start: s,
            end: e,
            all,
        };
        assert_eq!(get(during(25, 35, false), 0), Some(1.0));
        assert_eq!(get(during(25, 35, true), 0), Some(0.0));
        assert_eq!(get(during(30, 60, true), 0), Some(1.0));
        assert_eq!(get(during(10, 30, false), 0), Some(0.0));
    }
}
