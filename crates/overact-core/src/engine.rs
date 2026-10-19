//! Views (filtered + sorted row lists) and the incremental jobs that build them.
//!
//! A host submits a `ViewSpec` for a view id and then calls `step` repeatedly
//! with a work budget, yielding between calls. A newer submission for the
//! same view replaces a queued or running job, which is how cancellation
//! works without threads. Results only replace a view's rows when a job
//! finishes, so the host can keep drawing the previous result meanwhile.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::column::Column;
use crate::dataset::Dataset;
use crate::features::Feature;
use crate::filter::{self, Bound, ColRef, Condition, Test};
use crate::format::{Error, Result};
use crate::profile::Profiler;
use crate::raster::{self, Palette, RasterParams, RasterScratch};
use crate::sample::{Sample, Sampler};
use crate::sort::{RadixSorter, SortKey};

/// Largest value accepted in an `In` filter (category indices are u16).
pub const MAX_IN_VALUE: u32 = u16::MAX as u32;

/// Computed features that no view or job uses are kept (most recently used
/// first) up to this many bytes, so switching back to one is instant without
/// letting the cache grow forever (WASM memory never shrinks).
pub const UNUSED_FEATURE_BYTES: usize = 64 << 20;

/// A shared row list. Views and jobs share these, so reusing a filter result
/// for a new sort or sample copies nothing.
pub type Rows = Arc<Vec<u32>>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewSpec {
    #[serde(default)]
    pub filter: Vec<Condition>,
    #[serde(default)]
    pub sort: Vec<SortKey>,
    /// Random subsample of the filtered persons, taken before sorting.
    #[serde(default)]
    pub sample: Option<Sample>,
}

impl ViewSpec {
    pub fn columns(&self) -> impl Iterator<Item = &ColRef> {
        self.filter
            .iter()
            .map(|c| &c.col)
            .chain(self.sort.iter().map(|k| &k.col))
    }
}

pub struct View {
    pub spec: ViewSpec,
    /// Persons passing the filter, ascending.
    pub filtered: Rows,
    /// The sampled subset of `filtered`, ascending (None: all of them).
    pub sampled: Option<Rows>,
    /// `members()` in sort order.
    pub rows: Rows,
    /// Exact activity counts per minute, `[code * axis_len + minute]`.
    /// Computed after the rows are installed (None until then).
    pub profile: Option<Rows>,
    /// Increments every time the rows change.
    pub version: u32,
}

impl View {
    /// The persons shown: the sample if there is one, else all filtered.
    pub fn members(&self) -> &[u32] {
        self.sampled.as_ref().unwrap_or(&self.filtered)
    }
}

enum Stage {
    Virtual {
        feature: Feature,
        col: Box<Column>,
        pos: usize,
    },
    Filter {
        order: Vec<usize>,
        ci: usize,
        rows: Vec<u32>,
        pos: usize,
        w: usize,
    },
    Sample(Box<Sampler>),
    Sort(Box<RadixSorter>),
    Profile(Box<Profiler>),
    Done,
}

struct Job {
    view: u32,
    spec: ViewSpec,
    todo_virtual: Vec<Feature>,
    filtered: Option<Rows>,
    /// Some once sampling is done (Some(None): no sample).
    sampled: Option<Option<Rows>>,
    rows: Option<Rows>,
    profile: Option<Rows>,
    stage: Stage,
    /// Rough progress, 0..1.
    progress: f64,
    /// A follow-up job that only computes the profile of view `version`.
    profile_of: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    /// Views whose rows changed during this step.
    pub finished: Vec<u32>,
    /// Views whose profile became available during this step.
    pub profiled: Vec<u32>,
    /// Views still waiting for a job.
    pub pending: Vec<u32>,
    /// Progress of the running job (0..1).
    pub progress: f64,
}

pub struct Engine {
    pub ds: Dataset,
    virtuals: HashMap<Feature, Column>,
    /// When each computed feature was last asked for (a logical clock).
    feature_used: HashMap<Feature, u64>,
    clock: u64,
    /// Cache budget for unused features (default `UNUSED_FEATURE_BYTES`).
    pub unused_feature_bytes: usize,
    views: HashMap<u32, View>,
    jobs: VecDeque<Job>,
    scratch: RasterScratch,
    finished: Vec<u32>,
    profiled: Vec<u32>,
}

impl Engine {
    pub fn new(ds: Dataset) -> Engine {
        Engine {
            ds,
            virtuals: HashMap::new(),
            feature_used: HashMap::new(),
            clock: 0,
            unused_feature_bytes: UNUSED_FEATURE_BYTES,
            views: HashMap::new(),
            jobs: VecDeque::new(),
            scratch: RasterScratch::default(),
            finished: Vec::new(),
            profiled: Vec::new(),
        }
    }

    /// Attribute columns that must be loaded before `spec` can run.
    pub fn missing_columns(&self, spec: &ViewSpec) -> Vec<usize> {
        let mut out: Vec<usize> = spec
            .columns()
            .filter_map(|c| match c {
                ColRef::Attr { column } if !self.ds.column_loaded(*column) => Some(*column),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Looks up a column (attribute or computed feature).
    pub fn column(&self, c: &ColRef) -> Result<&Column> {
        match c {
            ColRef::Attr { column } => self
                .ds
                .column(*column)
                .ok_or_else(|| Error::Invalid(format!("column #{column} is not loaded"))),
            ColRef::Virtual(f) => self
                .virtuals
                .get(f)
                .ok_or_else(|| Error::Invalid("feature not computed yet".into())),
        }
    }

    /// Queues a job that builds view `view` from `spec`, replacing any queued
    /// job for the same view. Returns false if the view already shows `spec`
    /// (nothing is queued, so no `finished` event will follow).
    pub fn submit(&mut self, view: u32, spec: ViewSpec) -> Result<bool> {
        if !self.ds.schedules_ready() {
            return Err(Error::Invalid("schedules not loaded".into()));
        }
        let missing = self.missing_columns(&spec);
        if !missing.is_empty() {
            return Err(Error::Invalid(format!("columns not loaded: {missing:?}")));
        }
        for c in spec.columns() {
            if let ColRef::Attr { column } = c {
                if *column >= self.ds.header.columns.len() {
                    return Err(Error::UnknownColumn(format!("#{column}")));
                }
            }
        }
        for c in &spec.filter {
            // `In` builds a table up to the largest value; category indices
            // and booleans are far below this.
            if let Test::In { values, .. } = &c.test {
                if values.iter().any(|&v| v > MAX_IN_VALUE) {
                    return Err(Error::Invalid(format!(
                        "filter values must be at most {MAX_IN_VALUE}"
                    )));
                }
            }
        }
        if self.views.get(&view).is_some_and(|v| v.spec == spec) {
            // Already showing this spec: drop any other queued spec, but keep
            // the follow-up job computing this view's profile.
            self.jobs
                .retain(|j| j.view != view || j.profile_of.is_some());
            return Ok(false);
        }
        self.jobs.retain(|j| j.view != view);
        self.clock += 1;
        let mut todo_virtual: Vec<Feature> = Vec::new();
        for c in spec.columns() {
            if let ColRef::Virtual(f) = c {
                self.feature_used.insert(f.clone(), self.clock);
                if !self.virtuals.contains_key(f) && !todo_virtual.contains(f) {
                    todo_virtual.push(f.clone());
                }
            }
        }
        // Row jobs go ahead of follow-up profile jobs (which resume later).
        let at = self
            .jobs
            .iter()
            .position(|j| j.profile_of.is_some())
            .unwrap_or(self.jobs.len());
        self.jobs.insert(
            at,
            Job {
                view,
                spec,
                todo_virtual,
                filtered: None,
                sampled: None,
                rows: None,
                profile: None,
                stage: Stage::Done,
                progress: 0.0,
                profile_of: None,
            },
        );
        Ok(true)
    }

    pub fn drop_view(&mut self, view: u32) {
        self.jobs.retain(|j| j.view != view);
        self.views.remove(&view);
    }

    pub fn has_work(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub fn view(&self, view: u32) -> Option<&View> {
        self.views.get(&view)
    }

    /// Chooses the next stage of the front job after the previous one ended,
    /// finishing jobs (and starting the next one) as needed.
    fn start_next_stage(&mut self) {
        while let Some(job) = self.jobs.front() {
            let more = if job.profile_of.is_some() {
                self.next_profile_stage()
            } else {
                self.next_row_stage()
            };
            if more {
                return;
            }
        }
    }

    /// For a follow-up profile job: starts (or resumes) the profile, or
    /// installs the finished one. Returns true if the job has a stage to run.
    fn next_profile_stage(&mut self) -> bool {
        let job = self.jobs.front_mut().unwrap();
        if job.profile.is_none() {
            // Start the profile, or resume one that another job overtook.
            if matches!(job.stage, Stage::Done) {
                job.stage = Stage::Profile(Box::new(Profiler::new(&self.ds)));
            }
            return true;
        }
        let job = self.jobs.pop_front().unwrap();
        let version = job.profile_of.unwrap();
        if let Some(v) = self
            .views
            .get_mut(&job.view)
            .filter(|v| v.version == version)
        {
            v.profile = job.profile;
            self.profiled.push(job.view);
        }
        false
    }

    /// For a row job: starts its next stage (feature, filter, sample, sort),
    /// reusing results of the view's current spec where they still apply.
    /// Once the rows are known, installs them. Returns true if the job has a
    /// stage to run.
    fn next_row_stage(&mut self) -> bool {
        let n = self.ds.n_persons();
        let job = self.jobs.front_mut().unwrap();
        // Another job may have computed a feature meanwhile.
        job.todo_virtual.retain(|f| !self.virtuals.contains_key(f));
        if let Some(f) = job.todo_virtual.pop() {
            let col = Box::new(f.alloc(n).expect("feature dtype"));
            job.stage = Stage::Virtual {
                feature: f,
                col,
                pos: 0,
            };
            return true;
        }
        let old = self.views.get(&job.view);
        if job.filtered.is_none() {
            if let Some(v) = old.filter(|v| v.spec.filter == job.spec.filter) {
                job.filtered = Some(v.filtered.clone());
                if v.spec.sample == job.spec.sample {
                    job.sampled = Some(v.sampled.clone());
                    job.profile = v.profile.clone();
                }
            } else {
                job.stage = filter_stage(&self.ds, &self.virtuals, &job.spec.filter);
                return true;
            }
        }
        if job.sampled.is_none() {
            let len = job.filtered.as_ref().unwrap().len();
            match job.spec.sample {
                Some(s) if (s.n as usize) < len => {
                    let s = Sampler::new(len, s.n as usize, s.seed);
                    job.stage = Stage::Sample(Box::new(s));
                    return true;
                }
                _ => job.sampled = Some(None),
            }
        }
        if job.rows.is_none() {
            let filtered = job.filtered.as_ref().unwrap();
            let members = job.sampled.as_ref().unwrap().as_ref().unwrap_or(filtered);
            if let Some(v) = old.filter(|v| v.spec == job.spec) {
                job.rows = Some(v.rows.clone());
            } else if job.spec.sort.is_empty() {
                job.rows = Some(members.clone());
            } else {
                let cols: Vec<&Column> = job
                    .spec
                    .sort
                    .iter()
                    .map(|k| col_of(&self.ds, &self.virtuals, &k.col))
                    .collect();
                let s = RadixSorter::new(members.to_vec(), &job.spec.sort, &cols);
                job.stage = Stage::Sort(Box::new(s));
                return true;
            }
        }
        self.install();
        false
    }

    /// Installs the front job's rows as its view. If the profile is still
    /// needed the job stays queued as a follow-up that computes it.
    fn install(&mut self) {
        let mut job = self.jobs.pop_front().unwrap();
        let version = self.views.get(&job.view).map_or(0, |v| v.version + 1);
        let need_profile = job.profile.is_none();
        self.views.insert(
            job.view,
            View {
                spec: std::mem::take(&mut job.spec),
                filtered: job.filtered.take().unwrap(),
                sampled: job.sampled.take().unwrap(),
                rows: job.rows.take().unwrap(),
                profile: job.profile.take(),
                version,
            },
        );
        self.finished.push(job.view);
        if need_profile {
            job.profile_of = Some(version);
            job.stage = Stage::Done;
            job.progress = 0.95;
            self.jobs.push_front(job);
        } else {
            self.profiled.push(job.view);
        }
        self.evict_features();
    }

    /// Drops computed features beyond `UNUSED_FEATURE_BYTES` that no view or
    /// queued job uses, least recently used first.
    fn evict_features(&mut self) {
        let in_use = |f: &Feature| {
            let uses = |spec: &ViewSpec| {
                spec.columns()
                    .any(|c| matches!(c, ColRef::Virtual(g) if g == f))
            };
            self.views.values().any(|v| uses(&v.spec)) || self.jobs.iter().any(|j| uses(&j.spec))
        };
        let mut unused: Vec<(u64, Feature, usize)> = self
            .virtuals
            .iter()
            .filter(|(f, _)| !in_use(f))
            .map(|(f, c)| {
                let used = self.feature_used.get(f).copied().unwrap_or(0);
                (used, f.clone(), c.data.bytes().len())
            })
            .collect();
        unused.sort_by_key(|u| std::cmp::Reverse(u.0));
        let mut kept = 0;
        for (_, f, bytes) in unused {
            kept += bytes;
            if kept > self.unused_feature_bytes {
                self.virtuals.remove(&f);
                self.feature_used.remove(&f);
            }
        }
    }

    /// Whether a computed feature is cached (for tests and diagnostics).
    pub fn has_feature(&self, f: &Feature) -> bool {
        self.virtuals.contains_key(f)
    }

    /// Runs about `budget` element operations of queued work.
    pub fn step(&mut self, budget: usize) -> StepResult {
        let mut left = budget as i64;
        self.finished.clear();
        self.profiled.clear();
        while left > 0 && !self.jobs.is_empty() {
            let used = self.step_stage(left as usize);
            left -= used.max(1) as i64;
        }
        StepResult {
            finished: std::mem::take(&mut self.finished),
            profiled: std::mem::take(&mut self.profiled),
            pending: self.jobs.iter().map(|j| j.view).collect(),
            progress: self.jobs.front().map_or(1.0, |j| j.progress),
        }
    }

    /// Advances the front job's current stage. Returns the work done.
    fn step_stage(&mut self, budget: usize) -> usize {
        let n = self.ds.n_persons();
        let ds = &self.ds;
        let virtuals = &self.virtuals;
        let views = &self.views;
        let job = self.jobs.front_mut().unwrap();
        let mut stage_done = false;
        let used;
        match &mut job.stage {
            Stage::Done => {
                stage_done = true;
                used = 0;
            }
            Stage::Virtual { feature, col, pos } => {
                let end = (*pos + budget).min(n);
                feature.fill(ds, col, *pos, end);
                used = (end - *pos) * 4;
                *pos = end;
                job.progress = 0.3 * end as f64 / n.max(1) as f64;
                if end == n {
                    feature.finish(col);
                    stage_done = true;
                }
            }
            Stage::Filter {
                order,
                ci,
                rows,
                pos,
                w,
            } => {
                if order.is_empty() {
                    let end = (*pos + budget).min(n);
                    rows.extend(*pos as u32..end as u32);
                    used = end - *pos;
                    *pos = end;
                    stage_done = end == n;
                } else {
                    let cond = &job.spec.filter[order[*ci]];
                    let bound = Bound::new(cond, col_of(ds, virtuals, &cond.col));
                    if *ci == 0 {
                        let end = (*pos + budget).min(n);
                        filter::scan(&bound, *pos, end, rows);
                        used = end - *pos;
                        *pos = end;
                        if end == n {
                            *ci += 1;
                            *pos = 0;
                            *w = 0;
                        }
                    } else {
                        let len = rows.len();
                        let end = (*pos + budget).min(len);
                        *w = filter::refine(&bound, rows, *pos, end, *w);
                        used = end - *pos;
                        *pos = end;
                        if end == len {
                            rows.truncate(*w);
                            *ci += 1;
                            *pos = 0;
                            *w = 0;
                        }
                    }
                    job.progress = 0.3 + 0.3 * (*ci as f64 / order.len() as f64);
                    stage_done = *ci == order.len();
                }
                if stage_done {
                    let mut r = std::mem::take(rows);
                    r.shrink_to_fit();
                    job.filtered = Some(Arc::new(r));
                }
            }
            Stage::Sample(sampler) => {
                let rows = job.filtered.as_ref().map_or(&[][..], |r| &r[..]);
                stage_done = sampler.step(rows, budget);
                used = budget;
                job.progress = 0.6;
            }
            Stage::Sort(sorter) => {
                let cols: Vec<&Column> = job
                    .spec
                    .sort
                    .iter()
                    .map(|k| col_of(ds, virtuals, &k.col))
                    .collect();
                stage_done = sorter.step(&cols, budget);
                used = budget;
                job.progress = job.progress.max(0.6) + 0.02;
                job.progress = job.progress.min(0.9);
            }
            Stage::Profile(p) => {
                let rows = views.get(&job.view).map_or(&[][..], |v| v.members());
                let b = (budget / 8).max(1);
                stage_done = p.step(ds, rows, b);
                used = b * 8;
            }
        }
        if stage_done {
            let stage = std::mem::replace(&mut job.stage, Stage::Done);
            match stage {
                Stage::Virtual { feature, col, .. } => {
                    self.virtuals.insert(feature, *col);
                }
                Stage::Sample(s) => job.sampled = Some(Some(Arc::new(s.into_rows()))),
                Stage::Sort(s) => job.rows = Some(Arc::new(s.into_rows())),
                Stage::Profile(p) => job.profile = Some(Arc::new(p.finish())),
                _ => {}
            }
            self.start_next_stage();
        }
        used
    }

    /// Draws a view. Returns None if the view does not exist yet.
    pub fn raster(&mut self, view: u32, p: &RasterParams, pal: &Palette, out: &mut [u8]) -> bool {
        self.raster_band(view, p, pal, 0, p.height, out)
    }

    /// Draws pixel rows `y0..y1` of a view (see `raster::rasterize_band`).
    pub fn raster_band(
        &mut self,
        view: u32,
        p: &RasterParams,
        pal: &Palette,
        y0: u32,
        y1: u32,
        out: &mut [u8],
    ) -> bool {
        let Some(v) = self.views.get(&view) else {
            return false;
        };
        raster::rasterize_band(&self.ds, &v.rows, p, pal, &mut self.scratch, y0, y1, out);
        true
    }

    /// For each pixel row, the value of `col` for the person in the middle
    /// of that row (NaN if missing or empty).
    pub fn gutter(&self, view: u32, col: &ColRef, p: &RasterParams) -> Result<Vec<f64>> {
        let v = self
            .views
            .get(&view)
            .ok_or_else(|| Error::Invalid("no such view".into()))?;
        let c = self.column(col)?;
        Ok((0..p.height)
            .map(|y| {
                let (lo, hi) = raster::pixel_rows(p, y, v.rows.len());
                if hi > lo {
                    c.get(v.rows[(lo + hi) / 2] as usize).unwrap_or(f64::NAN)
                } else {
                    f64::NAN
                }
            })
            .collect())
    }

    /// Person index at row position `pos` of a view.
    pub fn person_at(&self, view: u32, pos: usize) -> Option<u32> {
        self.views.get(&view)?.rows.get(pos).copied()
    }
}

/// A filter stage with conditions ordered by estimated selectivity, most
/// selective first.
fn filter_stage(ds: &Dataset, virtuals: &HashMap<Feature, Column>, filter: &[Condition]) -> Stage {
    let n = ds.n_persons();
    let mut sel: Vec<(f64, usize)> = filter
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let col = col_of(ds, virtuals, &c.col);
            (Bound::new(c, col).selectivity(n), i)
        })
        .collect();
    sel.sort_by(|a, b| a.0.total_cmp(&b.0));
    Stage::Filter {
        order: sel.into_iter().map(|(_, i)| i).collect(),
        ci: 0,
        rows: Vec::new(),
        pos: 0,
        w: 0,
    }
}

fn col_of<'a>(ds: &'a Dataset, virtuals: &'a HashMap<Feature, Column>, c: &ColRef) -> &'a Column {
    match c {
        ColRef::Attr { column } => ds.column(*column).expect("checked in submit"),
        ColRef::Virtual(f) => virtuals.get(f).expect("computed before use"),
    }
}
