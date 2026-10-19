//! Tables to dataset.
//!
//! 1. Attributes (optional) are read first; their row order is the person order.
//! 2. Activities are streamed. While rows arrive grouped by person (the usual
//!    case) each person is cleaned and packed as soon as their rows end, so
//!    memory is about 2 bytes per segment. If a person's rows reappear later
//!    the activities are re-read in buffered mode: a counting pass, then every
//!    row scattered into per-person slots (about 8 bytes per row), then cleaned.
//! 3. Persons with activities but no attributes are sorted and appended;
//!    activity codes are ordered by total time; segments are copied into
//!    person order.

use crate::attrs::{infer, Builder, Encoded, Kind};
use crate::clean::{
    admit, clean_person, minutes_in_axis, ExampleJson, Fix, FixLog, Row, MAX_EXAMPLES,
};
use crate::ids::{hash, Ids};
use crate::table::{Batch, ColData, ColType, Table, BATCH_ROWS};
use crate::{Error, Progress, Result};
use overact_core::format::{
    pack_seg, seg_code, seg_start, Activity, BlobRef, Blobs, DType, Header, TimeAxis,
    MAX_ACTIVITIES, MAX_AXIS_LEN,
};
use overact_core::Dataset;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;

pub const UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit {
    #[default]
    Minute,
    Second,
    Hour,
}

impl TimeUnit {
    fn minutes(self) -> f64 {
        match self {
            TimeUnit::Minute => 1.0,
            TimeUnit::Second => 1.0 / 60.0,
            TimeUnit::Hour => 60.0,
        }
    }
}

/// Column roles and options (the `overact-pack` flags).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub id_col: String,
    pub act_col: String,
    pub start_col: String,
    /// Used unless `duration_col` is given.
    pub end_col: Option<String>,
    pub duration_col: Option<String>,
    /// Breaks ties between rows with the same start.
    pub seq_col: Option<String>,
    /// Person id column of the attributes table (default: `id_col`).
    pub attr_id_col: Option<String>,
    pub axis: (i32, i32),
    pub time_unit: TimeUnit,
    /// Attribute columns to keep (kept even if they look like identifiers).
    pub columns: Option<Vec<String>>,
    pub exclude: Vec<String>,
    /// Float attribute columns to store as f64 instead of f32.
    pub f64_cols: Vec<String>,
    pub store_ids: bool,
    /// Keep every fixed row in `Dataset::header.meta` (not just the first
    /// few of each kind), for `overact-pack --report`.
    pub report: bool,
    /// Rows used to infer CSV attribute types.
    pub infer_rows: usize,
    /// Keep a random share of persons (decided per id, so nested across fractions).
    pub fraction: f64,
    pub seed: u64,
    /// File names for the dataset's `meta.source`.
    pub activities_name: String,
    pub attributes_name: Option<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            id_col: "pid".into(),
            act_col: "act".into(),
            start_col: "start".into(),
            end_col: Some("end".into()),
            duration_col: None,
            seq_col: None,
            attr_id_col: None,
            axis: (0, 1440),
            time_unit: TimeUnit::Minute,
            columns: None,
            exclude: vec![],
            f64_cols: vec![],
            store_ids: true,
            report: false,
            infer_rows: 100_000,
            fraction: 1.0,
            seed: 1,
            activities_name: String::new(),
            attributes_name: None,
        }
    }
}

/// Decides which persons a random subset keeps.
struct Keep {
    thresh: Option<u64>,
    seed: u64,
}

impl Keep {
    fn new(fraction: f64, seed: u64) -> Keep {
        Keep {
            thresh: (fraction < 1.0).then(|| (fraction.max(0.0) * 18446744073709551616.0) as u64),
            seed: seed ^ 0x6f76_6572_6163_7421,
        }
    }

    #[inline]
    fn keeps(&self, id: &[u8]) -> bool {
        self.thresh.is_none_or(|t| hash(id, self.seed) < t)
    }
}

/// Activity labels with provisional codes (in order of first appearance).
#[derive(Default)]
struct Labels {
    /// Raw label bytes (labels need not be valid UTF-8), by code.
    keys: Vec<Vec<u8>>,
    names: Vec<String>,
    totals: Vec<i64>,
    /// Seen in the table (vs added only to fill gaps).
    from_file: Vec<bool>,
}

impl Labels {
    #[inline]
    fn code(&mut self, label: &[u8]) -> Result<u8> {
        // At most 32 short labels: a linear scan beats hashing.
        if let Some(c) = self.find(label) {
            self.from_file[c as usize] = true;
            return Ok(c);
        }
        let c = self.add(label)?;
        self.from_file[c as usize] = true;
        Ok(c)
    }

    #[inline]
    fn find(&self, label: &[u8]) -> Option<u8> {
        self.keys.iter().position(|k| k == label).map(|c| c as u8)
    }

    fn add(&mut self, label: &[u8]) -> Result<u8> {
        if self.names.len() >= MAX_ACTIVITIES {
            return Err(Error::new(format!(
                "more than {MAX_ACTIVITIES} activity labels (the format allows {MAX_ACTIVITIES}); \
                 is the activity column right?"
            )));
        }
        let c = self.names.len() as u8;
        self.keys.push(label.to_vec());
        self.names.push(String::from_utf8_lossy(label).into_owned());
        self.totals.push(0);
        self.from_file.push(false);
        Ok(c)
    }

    /// The code `unknown` has or would get next.
    fn unknown_code(&self) -> u8 {
        self.find(UNKNOWN.as_bytes())
            .unwrap_or(self.names.len() as u8)
    }

    /// Makes sure `unknown` exists (after it was used to fill a gap).
    fn use_unknown(&mut self) -> Result<u8> {
        match self.find(UNKNOWN.as_bytes()) {
            Some(c) => Ok(c),
            None => self.add(UNKNOWN.as_bytes()).map_err(|_| {
                Error::new(format!(
                    "gap filling needs an '{UNKNOWN}' activity but there are already \
                     {MAX_ACTIVITIES} labels"
                ))
            }),
        }
    }

    /// Final order: table labels by total time (descending, then label),
    /// then `unknown` if it was only added to fill gaps. Returns
    /// (labels in final order, provisional code -> final code).
    fn order(&self) -> (Vec<String>, Vec<u8>) {
        let mut idx: Vec<usize> = (0..self.names.len())
            .filter(|&i| self.from_file[i])
            .collect();
        idx.sort_by(|&a, &b| {
            self.totals[b]
                .cmp(&self.totals[a])
                .then_with(|| self.names[a].cmp(&self.names[b]))
        });
        idx.extend((0..self.names.len()).filter(|&i| !self.from_file[i]));
        let mut lut = vec![0u8; self.names.len()];
        for (new, &old) in idx.iter().enumerate() {
            lut[old] = new as u8;
        }
        (idx.iter().map(|&i| self.names[i].clone()).collect(), lut)
    }
}

/// Indices of the activity columns in the table.
struct SchedCols {
    id: usize,
    act: usize,
    start: usize,
    end: usize,
    is_duration: bool,
    seq: Option<usize>,
}

/// One parsed activity row.
struct Parsed<'a> {
    id: &'a [u8],
    act: &'a [u8],
    start: i32,
    end: i32,
    seq: i32,
}

/// Packed schedules in arrival order, addressed per person.
#[derive(Default)]
struct Packed {
    labels: Labels,
    log: FixLog,
    n_missing: u64,
    /// Per person: start in `store` (`u32::MAX` = no rows yet) and count.
    pos: Vec<u32>,
    cnt: Vec<u32>,
    store: Vec<u16>,
    scratch: Vec<(i32, u8)>,
}

impl Packed {
    fn grow(&mut self, n: usize) {
        if self.pos.len() < n {
            self.pos.resize(n, u32::MAX);
            self.cnt.resize(n, 0);
        }
    }

    /// Cleans and stores person `idx`'s rows.
    fn close(&mut self, idx: u32, rows: &mut [Row], axis: (i32, i32)) -> Result<()> {
        let unknown = self.labels.unknown_code();
        let fills = self.log.count(Fix::GapFilled) + self.log.count(Fix::PersonFilled);
        let start = self.store.len();
        if start + 2 * MAX_AXIS_LEN as usize > u32::MAX as usize {
            return Err(Error::new("more than 2^32 segments"));
        }
        let n = clean_person(
            rows,
            axis,
            unknown,
            idx,
            &mut self.log,
            &mut self.scratch,
            &mut self.store,
        );
        if self.log.count(Fix::GapFilled) + self.log.count(Fix::PersonFilled) > fills {
            self.labels.use_unknown()?;
        }
        self.pos[idx as usize] = start as u32;
        self.cnt[idx as usize] = n;
        Ok(())
    }
}

fn num_err(table: &Table, col: usize, v: &ColData, i: usize) -> Error {
    let mut buf = Vec::new();
    let text = v
        .key(i, &mut buf)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    Error::new(format!(
        "activities: column {:?} has a value that is not a number: {text:?}",
        table.names()[col]
    ))
}

/// Parses row `i` of an activities batch. `Ok(None)` if a field is missing.
#[inline]
fn parse_row<'a>(
    table: &Table,
    sc: &SchedCols,
    b: &'a Batch,
    i: usize,
    unit: f64,
    bufs: &'a mut [Vec<u8>; 2],
) -> Result<Option<Parsed<'a>>> {
    let [ib, ab] = bufs;
    let c = &b.cols;
    // Batch column k holds table column `src[k]`.
    let src = [sc.id, sc.act, sc.start, sc.end, sc.seq.unwrap_or(0)];
    // Infinite or NaN text is an error too, as in `overact-pack` (a NaN
    // float in Parquet is a missing value, see `ColData::num`).
    let num = |k: usize| match c[k].num(i) {
        Ok(x) if x.is_none_or(f64::is_finite) => Ok(x),
        _ => Err(num_err(table, src[k], &c[k], i)),
    };
    let (Some(start), Some(end)) = (num(2)?, num(3)?) else {
        return Ok(None);
    };
    let Some(id) = c[0].key(i, ib) else {
        return Ok(None);
    };
    let Some(act) = c[1].key(i, ab) else {
        return Ok(None);
    };
    let t = |x: f64| (x * unit).round_ties_even() as i64;
    let s = t(start);
    let e = if sc.is_duration { s + t(end) } else { t(end) };
    let clip = |x: i64| x.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    let seq = match sc.seq {
        Some(_) => num(4)?.map_or(i32::MIN, |x| clip(x as i64)),
        None => 0,
    };
    Ok(Some(Parsed {
        id,
        act,
        start: clip(s),
        end: clip(e),
        seq,
    }))
}

/// How to read the activities table.
struct Reader<'a> {
    table: &'a Table,
    sc: SchedCols,
    /// Table columns to scan, in batch order (id, act, start, end[, seq]).
    cols: Vec<usize>,
    /// Minutes per source time unit.
    unit: f64,
    axis: (i32, i32),
    keep: &'a Keep,
    report: bool,
}

impl<'a> Reader<'a> {
    fn new(table: &'a Table, cfg: &Config, keep: &'a Keep) -> Result<Reader<'a>> {
        let sc = SchedCols {
            id: table.column(&cfg.id_col)?,
            act: table.column(&cfg.act_col)?,
            start: table.column(&cfg.start_col)?,
            end: match &cfg.duration_col {
                Some(d) => table.column(d)?,
                None => table.column(cfg.end_col.as_deref().unwrap_or("end"))?,
            },
            is_duration: cfg.duration_col.is_some(),
            seq: cfg
                .seq_col
                .as_deref()
                .map(|s| table.column(s))
                .transpose()?,
        };
        let mut cols = vec![sc.id, sc.act, sc.start, sc.end];
        cols.extend(sc.seq);
        Ok(Reader {
            table,
            sc,
            cols,
            unit: cfg.time_unit.minutes(),
            axis: cfg.axis,
            keep,
            report: cfg.report,
        })
    }

    #[allow(clippy::type_complexity)]
    fn parts(&self) -> (&'a Table, &SchedCols, &[usize], f64, (i32, i32), &'a Keep) {
        (
            self.table, &self.sc, &self.cols, self.unit, self.axis, self.keep,
        )
    }

    fn packed(&self) -> Packed {
        let mut p = Packed::default();
        p.log.keep_all = self.report;
        p
    }

    /// Reads and cleans every person's activities: streaming if rows are
    /// grouped by person, else buffered. `ids` holds the persons with
    /// attributes; persons found only here are appended.
    fn read(&self, ids: &mut Ids, progress: Progress) -> Result<(Packed, &'static str)> {
        let n_attr = ids.len();
        if let Some(p) = read_grouped(self, ids, progress)? {
            return Ok((p, "grouped"));
        }
        ids.truncate(n_attr);
        Ok((read_buffered(self, ids, progress)?, "buffered"))
    }
}

/// Builds a dataset from an activities table and an optional attributes table.
pub fn ingest(
    activities: &Table,
    attributes: Option<&Table>,
    cfg: &Config,
    progress: Progress,
) -> Result<Dataset> {
    Ok(ingest_with_report(activities, attributes, cfg, progress)?.0)
}

/// `ingest`, also returning the fixed rows: every one with `cfg.report`,
/// else the first few of each kind (as in `meta.fix_examples`).
pub fn ingest_with_report(
    activities: &Table,
    attributes: Option<&Table>,
    cfg: &Config,
    progress: Progress,
) -> Result<(Dataset, Vec<ExampleJson>)> {
    let axis = cfg.axis;
    if !(1..=MAX_AXIS_LEN as i64).contains(&(axis.1 as i64 - axis.0 as i64)) {
        return Err(Error::new(format!(
            "time axis must be 1..{MAX_AXIS_LEN} minutes long"
        )));
    }
    let keep = Keep::new(cfg.fraction, cfg.seed);
    let mut warnings = Vec::new();
    let mut ids = Ids::new();

    // 1. Attributes.
    let (builders, attr_rows) = match attributes {
        Some(t) => read_attributes(t, cfg, &keep, &mut ids, &mut warnings, progress)?,
        None => (vec![], 0),
    };
    let n_attr = ids.len();

    // 2. Activities.
    let (mut packed, mode) = Reader::new(activities, cfg, &keep)?.read(&mut ids, progress)?;

    // 3. Finish. Lookups are done: free the id hash table first.
    ids.drop_table();
    progress("packing", 0, 1);
    let n = ids.len();
    if n == 0 {
        return Err(Error::new("no persons found"));
    }
    packed.grow(n);
    for idx in 0..n {
        if packed.pos[idx] == u32::MAX {
            packed.close(idx as u32, &mut [], axis)?;
        }
    }
    let n_extra = n - n_attr;
    if attributes.is_some() && n_extra > 0 {
        warnings.push(format!(
            "{n_extra} persons have activities but no attributes (their attributes are missing)"
        ));
    }
    // Persons without attributes go last, sorted by id.
    let mut order: Vec<u32> = (0..n as u32).collect();
    order[n_attr..].sort_unstable_by(|&a, &b| ids.get(a as usize).cmp(ids.get(b as usize)));
    let (labels, lut) = packed.labels.order();
    let (offsets, segments) = assemble(&mut packed, &order, &lut);

    let fixed = fixed_rows(&packed, &ids);
    let examples: Vec<&ExampleJson> = if cfg.report {
        first_of_each_kind(&fixed)
    } else {
        fixed.iter().collect()
    };
    let mut fixes = packed.log.to_json();
    if packed.n_missing > 0 {
        fixes.insert(Fix::DroppedMissing.name().into(), packed.n_missing.into());
    }
    let mut meta = json!({
        "loader": format!("overact-ingest {}", env!("CARGO_PKG_VERSION")),
        "source": { "activities": cfg.activities_name, "attributes": cfg.attributes_name },
        "fixes": fixes,
        "fix_examples": examples,
        "warnings": [],
        "extra_persons": n_extra,
        "mode": mode,
    });

    let person_ids = cfg.store_ids.then(|| {
        ids.permute(&order);
        ids.into_parts()
    });

    let mut columns: Vec<Encoded> = Vec::new();
    for b in builders {
        columns.extend(b.finish(n_extra, n_attr, &mut warnings));
    }
    meta["warnings"] = json!(warnings);
    if cfg.fraction < 1.0 {
        // Persons in the whole table: exact with attributes, estimated without.
        let of = if attributes.is_some() {
            attr_rows + (n_extra as f64 / cfg.fraction.max(1e-9)).round() as usize
        } else {
            (n as f64 / cfg.fraction.max(1e-9)).round() as usize
        };
        meta["subset"] = json!({ "fraction": cfg.fraction, "of": of, "seed": cfg.seed });
    }

    let header = header(axis, labels, &columns, meta);
    let data = columns.into_iter().map(|c| c.data).collect();
    progress("packing", 1, 1);
    let ds = Dataset::from_parts(header, offsets, segments, person_ids, data)?;
    Ok((ds, fixed))
}

/// Recodes activities to their final codes and lays segments out in person
/// `order`. Returns (offsets, segments) and frees the packed store.
fn assemble(packed: &mut Packed, order: &[u32], lut: &[u8]) -> (Vec<u32>, Vec<u16>) {
    let n = order.len();
    let recode = |s: u16| pack_seg(seg_start(s), lut[seg_code(s) as usize]);
    let mut offsets = Vec::with_capacity(n + 1);
    offsets.push(0u32);
    // Usually persons were stored in final order already (tables sorted
    // alike, no extras): then recode in place instead of copying.
    let in_order = order
        .iter()
        .enumerate()
        .all(|(k, &old)| old as usize == k && packed.pos[k] == offsets_at(packed, k));
    let segments = if in_order {
        let mut segs = std::mem::take(&mut packed.store);
        segs.iter_mut().for_each(|s| *s = recode(*s));
        for k in 0..n {
            offsets.push(packed.pos[k] + packed.cnt[k]);
        }
        segs
    } else {
        let mut segs = Vec::with_capacity(packed.store.len());
        for &old in order {
            let (p, c) = (
                packed.pos[old as usize] as usize,
                packed.cnt[old as usize] as usize,
            );
            segs.extend(packed.store[p..p + c].iter().map(|&s| recode(s)));
            offsets.push(segs.len() as u32);
        }
        drop(std::mem::take(&mut packed.store));
        segs
    };
    (offsets, segments)
}

/// The logged fixed rows with person ids and activity labels.
fn fixed_rows(packed: &Packed, ids: &Ids) -> Vec<ExampleJson> {
    packed
        .log
        .examples
        .iter()
        .map(|e| ExampleJson {
            fix: e.fix.name(),
            person_id: String::from_utf8_lossy(ids.get(e.person as usize)).into_owned(),
            start: e.start,
            end: e.end,
            activity: packed
                .labels
                .names
                .get(e.code as usize)
                .cloned()
                .unwrap_or_default(),
        })
        .collect()
}

/// The first `MAX_EXAMPLES` rows of each fix kind, in log order.
fn first_of_each_kind(rows: &[ExampleJson]) -> Vec<&ExampleJson> {
    let mut seen: Vec<(&str, usize)> = Vec::new();
    rows.iter()
        .filter(|r| {
            let k = match seen.iter().position(|s| s.0 == r.fix) {
                Some(k) => k,
                None => {
                    seen.push((r.fix, 0));
                    seen.len() - 1
                }
            };
            seen[k].1 += 1;
            seen[k].1 <= MAX_EXAMPLES
        })
        .collect()
}

/// The header of an ingested dataset (counts and blob layout are filled in
/// by `Dataset::from_parts`).
fn header(
    axis: (i32, i32),
    labels: Vec<String>,
    columns: &[Encoded],
    meta: serde_json::Value,
) -> Header {
    let blob = BlobRef {
        dtype: DType::U32,
        byte_offset: 0,
        byte_length: 0,
    };
    Header {
        format: "overact".into(),
        version: 1,
        n_persons: 0,
        n_segments: 0,
        time_axis: TimeAxis {
            start: axis.0,
            end: axis.1,
            unit: Some("minute".into()),
        },
        activities: labels
            .into_iter()
            .enumerate()
            .map(|(i, label)| Activity {
                code: i as u8,
                label,
            })
            .collect(),
        blobs: Blobs {
            offsets: blob.clone(),
            segments: blob,
            person_id_offsets: None,
            person_id_data: None,
        },
        columns: columns.iter().map(|c| c.meta.clone()).collect(),
        meta,
    }
}

/// Streams activities assuming rows are grouped by person. Returns None if
/// they turn out not to be.
fn read_grouped(r: &Reader, ids: &mut Ids, progress: Progress) -> Result<Option<Packed>> {
    let (table, sc, cols, unit, axis, keep) = r.parts();
    let mut p = r.packed();
    p.grow(ids.len());
    let mut cur: Option<u32> = None;
    let mut cur_id: Vec<u8> = Vec::new();
    let mut group: Vec<Row> = Vec::new();
    let mut ungrouped = false;
    let mut bufs = [Vec::new(), Vec::new()];
    let mut sized = false;
    let n_before = ids.len();
    table.scan(cols, &mut |b, (done, total)| {
        progress("activities", done, total);
        for i in 0..b.len {
            let Some(r) = parse_row(table, sc, b, i, unit, &mut bufs)? else {
                p.n_missing += 1;
                continue;
            };
            if !keep.keeps(r.id) {
                continue;
            }
            if !sized && i + 1 == b.len.min(BATCH_ROWS) {
                // New persons so far, extrapolated to the whole table.
                sized = true;
                ids.reserve(n_before + estimate(ids.len() - n_before, done, total));
                // Reserve the segment store up front: growing by doubling
                // would briefly need three times its size.
                p.store
                    .reserve(estimate(p.store.len() + group.len(), done, total));
            }
            let idx = match cur {
                Some(c) if r.id == cur_id.as_slice() => c,
                _ => {
                    if let Some(c) = cur {
                        p.close(c, &mut group, axis)?;
                        group.clear();
                    }
                    // Tables sorted the same way: the next person is usually
                    // the next index, which avoids a hash lookup.
                    let next = cur.map_or(0, |c| c as usize + 1);
                    let idx = if next < ids.len() && ids.get(next) == r.id {
                        next as u32
                    } else {
                        ids.insert(r.id)?.0
                    };
                    p.grow(idx as usize + 1);
                    if p.pos[idx as usize] != u32::MAX {
                        ungrouped = true;
                        return Ok(false);
                    }
                    cur_id.clear();
                    cur_id.extend_from_slice(r.id);
                    cur = Some(idx);
                    idx
                }
            };
            let code = p.labels.code(r.act)?;
            p.labels.totals[code as usize] += minutes_in_axis(r.start, r.end, axis);
            if let Some(row) = admit(r.start, r.end, r.seq, code, axis, idx, &mut p.log) {
                group.push(row);
            }
        }
        Ok(true)
    })?;
    if ungrouped {
        return Ok(None);
    }
    if let Some(c) = cur {
        p.close(c, &mut group, axis)?;
    }
    Ok(Some(p))
}

/// Where person `k`'s segments start if persons are stored back to back in
/// index order (`pos[k-1] + cnt[k-1]`).
#[inline]
fn offsets_at(p: &Packed, k: usize) -> u32 {
    if k == 0 {
        0
    } else {
        p.pos[k - 1] + p.cnt[k - 1]
    }
}

/// Extrapolates a count seen in the first `done` of `total` (bytes or rows),
/// with 10% to spare.
fn estimate(seen: usize, done: u64, total: u64) -> usize {
    if done == 0 {
        return seen;
    }
    ((seen as f64 * total as f64 / done as f64) * 1.1) as usize
}

/// A buffered activity row: raw start, clipped end (minutes from the axis
/// start) and provisional code.
#[derive(Clone, Copy, Default)]
struct BufRow {
    start: i32,
    end: u16,
    code: u8,
}

/// Reads activities in any row order: count rows per person, scatter them
/// into per-person slots, then clean each person.
fn read_buffered(r: &Reader, ids: &mut Ids, progress: Progress) -> Result<Packed> {
    let (table, sc, cols, unit, axis, keep) = r.parts();
    // Pass 1: rows per person. Persons with attributes come first.
    let n_attr = ids.len();
    let mut count: Vec<u32> = vec![0; ids.len()];
    let mut buf = Vec::new();
    table.scan(&cols[..1], &mut |b, (done, total)| {
        progress("activities (regrouping)", done, total);
        for i in 0..b.len {
            let Some(id) = b.cols[0].key(i, &mut buf) else {
                continue;
            };
            if !keep.keeps(id) {
                continue;
            }
            let (idx, _) = ids.insert(id)?;
            if idx as usize >= count.len() {
                count.resize(idx as usize + 1, 0);
            }
            count[idx as usize] += 1;
        }
        Ok(true)
    })?;
    let n = ids.len();
    let mut start = Vec::with_capacity(n + 1);
    let mut total = 0u64;
    for &c in &count {
        start.push(total as u32);
        total += c as u64;
    }
    if total > u32::MAX as u64 {
        return Err(Error::new("more than 2^32 activity rows"));
    }
    start.push(total as u32);
    let mut fill = count;
    fill.iter_mut().for_each(|x| *x = 0);
    let mut rows = vec![BufRow::default(); total as usize];
    let mut seqs = if sc.seq.is_some() {
        vec![0i32; total as usize]
    } else {
        vec![]
    };

    // Pass 2: scatter. `seen` marks persons without attributes that have a
    // complete row (pass 1 only looked at ids).
    let mut p = r.packed();
    p.grow(n);
    let mut seen = vec![false; n - n_attr];
    let mut bufs = [Vec::new(), Vec::new()];
    table.scan(cols, &mut |b, (done, total)| {
        progress("activities (second pass)", done, total);
        for i in 0..b.len {
            let Some(r) = parse_row(table, sc, b, i, unit, &mut bufs)? else {
                p.n_missing += 1;
                continue;
            };
            if !keep.keeps(r.id) {
                continue;
            }
            let idx = ids
                .find(r.id)
                .ok_or_else(|| Error::new("the activities table changed while reading"))?;
            if let Some(s) = seen.get_mut((idx as usize).wrapping_sub(n_attr)) {
                *s = true;
            }
            let code = p.labels.code(r.act)?;
            p.labels.totals[code as usize] += minutes_in_axis(r.start, r.end, axis);
            if let Some(row) = admit(r.start, r.end, r.seq, code, axis, idx, &mut p.log) {
                let k = (start[idx as usize] + fill[idx as usize]) as usize;
                fill[idx as usize] += 1;
                rows[k] = BufRow {
                    start: row.start,
                    end: (row.end - axis.0) as u16,
                    code: row.code,
                };
                if !seqs.is_empty() {
                    seqs[k] = row.seq;
                }
            }
        }
        Ok(true)
    })?;

    // Persons whose rows all lack a field: `overact-pack` (and grouped mode)
    // drop such rows before persons are found, so drop the persons too.
    let mut map = None;
    if seen.contains(&false) {
        let keep: Vec<bool> = (0..n).map(|i| i < n_attr || seen[i - n_attr]).collect();
        let m = ids.retain(&keep);
        // No fix was logged for a dropped person (none of its rows parsed).
        for e in &mut p.log.examples {
            e.person = m[e.person as usize];
        }
        p.pos.truncate(ids.len());
        p.cnt.truncate(ids.len());
        map = Some(m);
    }
    drop(seen);

    // Clean each person.
    let mut group = Vec::new();
    for old in 0..n {
        let idx = match &map {
            Some(m) if m[old] == u32::MAX => continue,
            Some(m) => m[old] as usize,
            None => old,
        };
        let a = start[old] as usize;
        group.clear();
        group.extend((a..a + fill[old] as usize).map(|k| Row {
            start: rows[k].start,
            seq: seqs.get(k).copied().unwrap_or(0),
            end: rows[k].end as i32 + axis.0,
            code: rows[k].code,
        }));
        p.close(idx as u32, &mut group, axis)?;
        if old % (1 << 16) == 0 {
            progress("cleaning", old as u64, n as u64);
        }
    }
    Ok(p)
}

/// Reads the attributes table into column builders and the id table.
/// Returns the builders and the number of attribute rows (before any subset).
fn read_attributes(
    table: &Table,
    cfg: &Config,
    keep: &Keep,
    ids: &mut Ids,
    warnings: &mut Vec<String>,
    progress: Progress,
) -> Result<(Vec<Builder>, usize)> {
    let id_name = cfg.attr_id_col.as_deref().unwrap_or(&cfg.id_col);
    let id_col = table.column(id_name)?;
    let names = table.names();
    let mut picked: Vec<usize> = (0..names.len()).filter(|&c| c != id_col).collect();
    if let Some(want) = &cfg.columns {
        let unknown: Vec<&String> = want
            .iter()
            .filter(|w| !picked.iter().any(|&c| &names[c] == *w))
            .collect();
        if !unknown.is_empty() {
            return Err(Error::new(format!(
                "attributes: unknown columns {unknown:?}"
            )));
        }
        picked.retain(|&c| want.contains(&names[c]));
    }
    picked.retain(|&c| !cfg.exclude.contains(&names[c]));
    let types = table.types();
    picked.retain(|&c| {
        let ok = types[c] != ColType::Unsupported;
        if !ok {
            warnings.push(format!("column {:?}: unsupported type; skipped", names[c]));
        }
        ok
    });
    let forced = |c: usize| cfg.columns.as_ref().is_some_and(|w| w.contains(&names[c]));
    let wide = |c: usize| cfg.f64_cols.contains(&names[c]);
    let mut cols = vec![id_col];
    cols.extend(&picked);

    let mut as_text: HashSet<usize> = HashSet::new();
    loop {
        *ids = Ids::with_capacity(table.n_rows().unwrap_or(0) as usize);
        let mut local_warnings = Vec::new();
        let mut builders: Option<Vec<Builder>> = None;
        if table.format() == crate::table::Format::Parquet {
            builders = Some(
                picked
                    .iter()
                    .map(|&c| {
                        let kind = match types[c] {
                            ColType::Int => Kind::Int,
                            ColType::Float => Kind::Float,
                            ColType::Bool => Kind::Bool,
                            _ => Kind::Text,
                        };
                        Builder::new(names[c].clone(), kind, forced(c), wide(c))
                    })
                    .collect(),
            );
        }
        let mut pending: Vec<Batch> = Vec::new();
        let mut pending_rows = 0;
        let mut retry: Option<usize> = None;
        let mut rows = 0usize;
        let mut buf = Vec::new();
        let mut idbuf = Vec::new();

        // `size`: ids expected in the whole table (0: no estimate).
        let mut process = |b: &Batch,
                           size: usize,
                           builders: &mut Vec<Builder>,
                           retry: &mut Option<usize>,
                           w: &mut Vec<String>|
         -> Result<bool> {
            ids.reserve(size);
            for i in 0..b.len {
                rows += 1;
                let Some(id) = b.cols[0].key(i, &mut idbuf) else {
                    return Err(Error::new(format!(
                        "attributes: data row {rows} has no {id_name}"
                    )));
                };
                if !keep.keeps(id) {
                    continue;
                }
                let (_, new) = ids.insert(id)?;
                if !new {
                    return Err(Error::new(format!(
                        "attributes: duplicate person id {:?} (each person needs exactly one row)",
                        String::from_utf8_lossy(id)
                    )));
                }
                for (k, bl) in builders.iter_mut().enumerate() {
                    if bl.push(&b.cols[k + 1], i, &mut buf, w).is_err() {
                        *retry = Some(picked[k]);
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        };
        let infer_all = |pending: &[Batch]| -> Vec<Builder> {
            picked
                .iter()
                .enumerate()
                .map(|(k, &c)| {
                    let kind = if as_text.contains(&c) {
                        Kind::Text
                    } else {
                        let vals = pending.iter().flat_map(|b| {
                            let ColData::Text(t) = &b.cols[k + 1] else {
                                unreachable!()
                            };
                            (0..t.len()).map(move |i| t.get(i))
                        });
                        infer(vals.take(cfg.infer_rows.max(1)))
                    };
                    Builder::new(names[c].clone(), kind, forced(c), wide(c))
                })
                .collect()
        };

        // Ids expected in the table, extrapolated from the first batch and
        // handed to the first `process` call.
        let (mut first, mut est) = (true, 0);
        table.scan(&cols, &mut |b, (done, total)| {
            progress("attributes", done, total);
            if std::mem::take(&mut first) {
                est = estimate(b.len, done, total);
            }
            if builders.is_none() {
                pending_rows += b.len;
                pending.push(b.clone());
                if pending_rows < cfg.infer_rows {
                    return Ok(true);
                }
                let mut bl = infer_all(&pending);
                for pb in std::mem::take(&mut pending) {
                    if !process(
                        &pb,
                        std::mem::take(&mut est),
                        &mut bl,
                        &mut retry,
                        &mut local_warnings,
                    )? {
                        return Ok(false);
                    }
                }
                builders = Some(bl);
                return Ok(true);
            }
            process(
                b,
                std::mem::take(&mut est),
                builders.as_mut().unwrap(),
                &mut retry,
                &mut local_warnings,
            )
        })?;
        if builders.is_none() && retry.is_none() {
            let mut bl = infer_all(&pending);
            for pb in std::mem::take(&mut pending) {
                if !process(&pb, 0, &mut bl, &mut retry, &mut local_warnings)? {
                    break;
                }
            }
            builders = Some(bl);
        }
        match retry {
            Some(c) => {
                as_text.insert(c);
            }
            None => {
                warnings.extend(local_warnings);
                return Ok((builders.unwrap(), rows));
            }
        }
    }
}
