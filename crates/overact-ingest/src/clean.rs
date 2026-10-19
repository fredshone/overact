//! Per-person schedule cleaning: rows are sorted by start (then seq), clipped to
//! the time axis, overlaps trimmed or dropped, gaps filled with `unknown`,
//! Back-to-back rows of the same activity stay separate segments. Every fix
//! is counted.

use overact_core::format::pack_seg;
use serde::Serialize;

/// Fix kinds, in the packer's naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    DroppedMissing,
    DroppedEmpty,
    DroppedOutside,
    Clipped,
    OverlapTrimmed,
    OverlapDropped,
    GapFilled,
    PersonFilled,
}

pub const FIXES: [Fix; 8] = [
    Fix::DroppedMissing,
    Fix::DroppedEmpty,
    Fix::DroppedOutside,
    Fix::Clipped,
    Fix::OverlapTrimmed,
    Fix::OverlapDropped,
    Fix::GapFilled,
    Fix::PersonFilled,
];

impl Fix {
    pub fn name(self) -> &'static str {
        match self {
            Fix::DroppedMissing => "dropped_missing",
            Fix::DroppedEmpty => "dropped_empty",
            Fix::DroppedOutside => "dropped_outside",
            Fix::Clipped => "clipped",
            Fix::OverlapTrimmed => "overlap_trimmed",
            Fix::OverlapDropped => "overlap_dropped",
            Fix::GapFilled => "gap_filled",
            Fix::PersonFilled => "person_filled",
        }
    }
}

/// Examples kept per fix kind.
pub const MAX_EXAMPLES: usize = 20;

/// One fixed row, for the report. Times are absolute minutes.
#[derive(Debug, Clone, Copy)]
pub struct Example {
    pub fix: Fix,
    pub person: u32,
    pub start: i64,
    pub end: i64,
    pub code: u8,
}

/// Counts every fix and keeps the first few of each kind (or, with
/// `keep_all`, every one).
#[derive(Debug, Default, Clone)]
pub struct FixLog {
    pub counts: [u64; 8],
    pub examples: Vec<Example>,
    pub keep_all: bool,
}

impl FixLog {
    #[inline]
    pub fn add(&mut self, fix: Fix, person: u32, start: i64, end: i64, code: u8) {
        let k = fix as usize;
        if self.keep_all || (self.counts[k] as usize) < MAX_EXAMPLES {
            self.examples.push(Example {
                fix,
                person,
                start,
                end,
                code,
            });
        }
        self.counts[k] += 1;
    }

    pub fn count(&self, fix: Fix) -> u64 {
        self.counts[fix as usize]
    }

    /// Non-zero counts by name.
    pub fn to_json(&self) -> serde_json::Map<String, serde_json::Value> {
        FIXES
            .iter()
            .filter(|f| self.count(**f) > 0)
            .map(|f| (f.name().to_string(), self.count(*f).into()))
            .collect()
    }
}

/// A row that passed `admit`: raw start (the sort key), seq, clipped end, code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub start: i32,
    pub seq: i32,
    pub end: i32,
    pub code: u8,
}

/// Row-level checks that don't depend on other rows: drops empty rows and
/// rows outside the axis, counts clipped ones. Returns the row to keep.
#[inline]
pub fn admit(
    start: i32,
    end: i32,
    seq: i32,
    code: u8,
    axis: (i32, i32),
    person: u32,
    log: &mut FixLog,
) -> Option<Row> {
    let (a0, a1) = axis;
    if start >= end {
        log.add(Fix::DroppedEmpty, person, start as i64, end as i64, code);
        return None;
    }
    let (s, e) = (start.clamp(a0, a1), end.clamp(a0, a1));
    if s >= e {
        log.add(Fix::DroppedOutside, person, start as i64, end as i64, code);
        return None;
    }
    if s != start || e != end {
        log.add(Fix::Clipped, person, start as i64, end as i64, code);
    }
    Some(Row {
        start,
        seq,
        end: e,
        code,
    })
}

/// Cleans one person's admitted rows (in file order) and appends the packed
/// segments to `out`. Returns the number of segments (at least 1).
/// `scratch` is reused between calls.
pub fn clean_person(
    rows: &mut [Row],
    axis: (i32, i32),
    unknown: u8,
    person: u32,
    log: &mut FixLog,
    scratch: &mut Vec<(i32, u8)>,
    out: &mut Vec<u16>,
) -> u32 {
    let (a0, a1) = axis;
    rows.sort_by_key(|r| (r.start, r.seq));
    let segs = scratch;
    segs.clear();
    let mut max_end: Option<i32> = None;
    let mut last_end: Option<i32> = None;
    for r in rows.iter() {
        let mut s = r.start.clamp(a0, a1);
        if let Some(pe) = max_end {
            if s < pe {
                if pe >= r.end {
                    log.add(Fix::OverlapDropped, person, s as i64, r.end as i64, r.code);
                    continue;
                }
                log.add(Fix::OverlapTrimmed, person, s as i64, r.end as i64, r.code);
                s = pe;
            }
        }
        max_end = Some(max_end.map_or(r.end, |m| m.max(r.end)));
        let gap_start = last_end.unwrap_or(a0);
        if s > gap_start {
            log.add(Fix::GapFilled, person, gap_start as i64, s as i64, unknown);
            segs.push((gap_start, unknown));
        }
        segs.push((s, r.code));
        last_end = Some(r.end);
    }
    match last_end {
        None => {
            log.add(Fix::PersonFilled, person, a0 as i64, a1 as i64, unknown);
            segs.push((a0, unknown));
        }
        Some(e) if e < a1 => {
            log.add(Fix::GapFilled, person, e as i64, a1 as i64, unknown);
            segs.push((e, unknown));
        }
        _ => {}
    }
    out.extend(segs.iter().map(|&(s, c)| pack_seg((s - a0) as u16, c)));
    segs.len() as u32
}

/// Minutes of `[start, end)` inside the axis (for ordering activities).
#[inline]
pub fn minutes_in_axis(start: i32, end: i32, axis: (i32, i32)) -> i64 {
    (end.min(axis.1) as i64 - start.max(axis.0) as i64).max(0)
}

#[derive(Serialize)]
pub struct ExampleJson {
    pub fix: &'static str,
    pub person_id: String,
    pub start: i64,
    pub end: i64,
    pub activity: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use overact_core::format::{seg_code, seg_start};

    fn run(rows: &[(i32, i32, u8)], axis: (i32, i32)) -> (Vec<(u16, u8)>, FixLog) {
        let mut log = FixLog::default();
        let mut kept: Vec<Row> = rows
            .iter()
            .filter_map(|&(s, e, c)| admit(s, e, 0, c, axis, 0, &mut log))
            .collect();
        let mut out = vec![];
        clean_person(&mut kept, axis, 9, 0, &mut log, &mut vec![], &mut out);
        (
            out.iter().map(|&x| (seg_start(x), seg_code(x))).collect(),
            log,
        )
    }

    #[test]
    fn tiles_and_logs() {
        let (segs, log) = run(
            &[(0, 480, 0), (470, 1000, 1), (900, 950, 2), (1100, 1500, 0)],
            (0, 1440),
        );
        assert_eq!(segs, vec![(0, 0), (480, 1), (1000, 9), (1100, 0)]);
        assert_eq!(log.count(Fix::OverlapTrimmed), 1);
        assert_eq!(log.count(Fix::OverlapDropped), 1);
        assert_eq!(log.count(Fix::GapFilled), 1);
        assert_eq!(log.count(Fix::Clipped), 1);
    }

    #[test]
    fn empty_person_and_repeats() {
        let (segs, log) = run(&[(10, 10, 0), (2000, 3000, 1)], (0, 1440));
        assert_eq!(segs, vec![(0, 9)]);
        assert_eq!(log.count(Fix::DroppedEmpty), 1);
        assert_eq!(log.count(Fix::DroppedOutside), 1);
        assert_eq!(log.count(Fix::PersonFilled), 1);
        let (segs, log) = run(&[(0, 100, 1), (100, 200, 1), (300, 1440, 9)], (0, 1440));
        // Back-to-back repeats stay separate, and so does a gap next to `unknown`.
        assert_eq!(segs, vec![(0, 1), (100, 1), (200, 9), (300, 9)]);
        assert_eq!(log.count(Fix::GapFilled), 1);
    }

    #[test]
    fn shifted_axis() {
        let (segs, _) = run(&[(240, 600, 0), (600, 1680, 1)], (240, 1680));
        assert_eq!(segs, vec![(0, 0), (360, 1)]);
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use overact_core::format::{seg_code, seg_start};
    use proptest::prelude::*;

    /// Reference: each minute takes the first row (in (start, seq) order,
    /// file order on ties) that covers it, else a gap; then each run of one
    /// row (or of one gap) is a segment, gaps coded `unknown`.
    fn painter(rows: &[(i32, i32, i32, u8)], axis: (i32, i32), unknown: u8) -> Vec<(u16, u8)> {
        let mut sorted: Vec<_> = rows.to_vec();
        sorted.sort_by_key(|r| (r.0, r.2));
        let len = (axis.1 - axis.0) as usize;
        let mut grid = vec![None; len];
        for (i, &(s, e, _, _)) in sorted.iter().enumerate() {
            for t in s.max(axis.0)..e.min(axis.1) {
                let m = &mut grid[(t - axis.0) as usize];
                if m.is_none() {
                    *m = Some(i);
                }
            }
        }
        let mut out: Vec<(u16, u8)> = vec![];
        for t in 0..len {
            if t == 0 || grid[t] != grid[t - 1] {
                out.push((t as u16, grid[t].map_or(unknown, |i| sorted[i].3)));
            }
        }
        out
    }

    proptest! {
        #[test]
        fn clean_matches_painter(
            rows in prop::collection::vec((-50i32..250, 0i32..120, 0i32..3, 0u8..4), 0..12),
            shift in -20i32..40,
        ) {
            let axis = (shift, shift + 200);
            let rows: Vec<(i32, i32, i32, u8)> = rows.into_iter().map(|(s, d, q, c)| (s, s + d - 10, q, c)).collect();
            let mut log = FixLog::default();
            let mut kept: Vec<Row> = rows
                .iter()
                .filter_map(|&(s, e, q, c)| admit(s, e, q, c, axis, 0, &mut log))
                .collect();
            let mut out = vec![];
            let n = clean_person(&mut kept, axis, 7, 0, &mut log, &mut vec![], &mut out);
            prop_assert_eq!(n as usize, out.len());
            let got: Vec<(u16, u8)> = out.iter().map(|&x| (seg_start(x), seg_code(x))).collect();
            prop_assert_eq!(got, painter(&rows, axis, 7));
        }
    }
}
