//! Rasterises sorted schedules into an RGBA pixel buffer.
//!
//! Each pixel row covers `step = (row_end - row_start) / height` persons of
//! the (filtered, sorted) row list. If a row covers at most `max_samples`
//! persons they are all used, otherwise `max_samples` evenly strided samples.
//! Segments are painted with difference arrays over pixel columns, so the
//! cost is O(segments + width) per pixel row, independent of zoom.

use crate::dataset::Dataset;
use crate::format::{seg_code, seg_start, MAX_ACTIVITIES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterMode {
    /// Average colour in linear RGB.
    Mean,
    /// The colour of the activity with the most coverage.
    Mode,
    /// Like `Mode`, but only from the first row in each pixel row
    /// (`max_samples` is ignored).
    Sample,
}

#[derive(Debug, Clone)]
pub struct RasterParams {
    pub width: u32,
    pub height: u32,
    /// Visible rows `[row_start, row_end)` of the row list (may be fractional).
    pub row_start: f64,
    pub row_end: f64,
    /// Visible time `[t0, t1)` in minutes from the axis start (may exceed the axis).
    pub t0: f64,
    pub t1: f64,
    /// Max persons sampled per pixel row; 0 means all.
    pub max_samples: u32,
    pub mode: RasterMode,
}

#[derive(Debug, Clone)]
pub struct Palette {
    /// Linear RGB per activity code.
    pub colors: [[f32; 3]; MAX_ACTIVITIES],
    /// Linear RGB of the background (outside the axis, empty rows).
    pub background: [f32; 3],
}

pub fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

impl Palette {
    /// Builds a palette from sRGB colours (`0xRRGGBB`).
    pub fn from_srgb(colors: &[u32], background: u32) -> Palette {
        let lin = |c: u32| {
            [
                srgb_to_linear((c >> 16) as u8),
                srgb_to_linear((c >> 8) as u8),
                srgb_to_linear(c as u8),
            ]
        };
        let mut out = [[0.0; 3]; MAX_ACTIVITIES];
        for (o, &c) in out.iter_mut().zip(colors) {
            *o = lin(c);
        }
        Palette {
            colors: out,
            background: lin(background),
        }
    }
}

/// Lookup table from linear [0, 1] (4096 steps) to sRGB bytes.
pub struct SrgbLut(Vec<u8>);

impl Default for SrgbLut {
    fn default() -> Self {
        SrgbLut(
            (0..4096)
                .map(|i| linear_to_srgb(i as f32 / 4095.0))
                .collect(),
        )
    }
}

impl SrgbLut {
    #[inline(always)]
    pub fn get(&self, c: f32) -> u8 {
        // Clamping the integer index is about twice as fast as clamping the float.
        let i = (c * 4095.0 + 0.5) as i32;
        self.0[i.clamp(0, 4095) as usize]
    }
}

/// The range of row-list positions drawn in pixel row `y`, and whether it is a
/// single representative row (zoomed in further than one row per pixel).
#[inline]
pub fn pixel_rows(p: &RasterParams, y: u32, n_rows: usize) -> (usize, usize) {
    let step = (p.row_end - p.row_start) / p.height as f64;
    let a = p.row_start + y as f64 * step;
    let lo = a.floor().max(0.0) as usize;
    let hi = ((a + step).floor().max(0.0) as usize).min(n_rows);
    if hi > lo {
        (lo, hi)
    } else {
        let c = (a + step * 0.5).floor();
        if c >= 0.0 && (c as usize) < n_rows {
            (c as usize, c as usize + 1)
        } else {
            (0, 0)
        }
    }
}

/// Positions sampled from `[lo, hi)`: all of them, or `k` evenly strided.
#[inline]
pub fn sample_positions(lo: usize, hi: usize, k: u32) -> impl Iterator<Item = usize> {
    let cnt = hi - lo;
    let k = if k == 0 || cnt <= k as usize {
        cnt
    } else {
        k as usize
    };
    (0..k).map(move |j| lo + ((2 * j + 1) * cnt) / (2 * k))
}

/// Reusable scratch buffers for `rasterize`.
#[derive(Default)]
pub struct RasterScratch {
    acc: Vec<f32>,
    cov: Vec<f32>,
    lut: SrgbLut,
}

/// Fraction of pixel `x` that overlaps the data `[0, axis_len)`.
fn axis_coverage(x: usize, x0: f64, x1: f64) -> f32 {
    let a = (x as f64).max(x0);
    let b = ((x + 1) as f64).min(x1);
    (b - a).max(0.0) as f32
}

/// Draws `rows` (person indices) into `out` (RGBA, `width * height * 4` bytes).
pub fn rasterize(
    ds: &Dataset,
    rows: &[u32],
    p: &RasterParams,
    pal: &Palette,
    scratch: &mut RasterScratch,
    out: &mut [u8],
) {
    rasterize_band(ds, rows, p, pal, scratch, 0, p.height, out);
}

/// Draws pixel rows `y0..y1` only, into `out` (`width * (y1 - y0) * 4` bytes).
///
/// Per pixel row, each sampled segment adds its colour times its coverage of
/// each pixel column to a difference array `acc` (interleaved channels, one
/// slot per column plus one): a segment covering `[xs, xe)` touches at most
/// four slots, whatever its length. A running sum then gives each pixel.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_band(
    ds: &Dataset,
    rows: &[u32],
    p: &RasterParams,
    pal: &Palette,
    scratch: &mut RasterScratch,
    y0: u32,
    y1: u32,
    out: &mut [u8],
) {
    let w = p.width as usize;
    let (y0, y1) = (y0.min(p.height) as usize, y1.min(p.height) as usize);
    assert_eq!(out.len(), w * (y1 - y0) * 4);
    if out.is_empty() {
        return;
    }
    let len = ds.axis_len() as f64;
    let scale = w as f64 / (p.t1 - p.t0);
    // Data extent in pixel coordinates.
    let (dx0, dx1) = ((0.0 - p.t0) * scale, (len - p.t0) * scale);
    scratch.cov.clear();
    scratch
        .cov
        .extend((0..w).map(|x| axis_coverage(x, dx0, dx1)));
    let ch = match p.mode {
        RasterMode::Mean => 3,
        RasterMode::Mode | RasterMode::Sample => ds.n_activities().max(1),
    };
    let lut = &scratch.lut;
    let bg = pal.background;
    let painter = RowPainter {
        starts: &ds.offsets,
        segs: &ds.segments,
        len16: ds.axis_len() as u16,
        rows,
        p,
        pal,
        cov: &scratch.cov,
        lut,
        ch,
        w,
        t0: p.t0 as f32,
        scale: scale as f32,
        bg_px: [lut.get(bg[0]), lut.get(bg[1]), lut.get(bg[2]), 255],
    };

    #[cfg(feature = "parallel")]
    if (y1 - y0) >= 8 && rayon::current_num_threads() > 1 {
        use rayon::prelude::*;
        const BAND: usize = 4;
        out.par_chunks_mut(w * 4 * BAND).enumerate().for_each_init(
            || vec![0.0f32; ch * (w + 2)],
            |acc, (bi, chunk)| {
                for (k, line) in chunk.chunks_mut(w * 4).enumerate() {
                    painter.draw_row(y0 + bi * BAND + k, line, acc);
                }
            },
        );
        return;
    }

    let acc = &mut scratch.acc;
    acc.resize(ch * (w + 2), 0.0);
    for (y, line) in (y0..y1).zip(out.chunks_mut(w * 4)) {
        painter.draw_row(y, line, acc);
    }
}

/// Everything needed to draw one pixel row; shared read-only across threads.
struct RowPainter<'a> {
    starts: &'a [u32],
    segs: &'a [u16],
    len16: u16,
    rows: &'a [u32],
    p: &'a RasterParams,
    pal: &'a Palette,
    cov: &'a [f32],
    lut: &'a SrgbLut,
    ch: usize,
    w: usize,
    t0: f32,
    scale: f32,
    bg_px: [u8; 4],
}

impl RowPainter<'_> {
    /// Draws pixel row `y` into `line` (`w * 4` bytes), using `acc` as scratch.
    fn draw_row(&self, y: usize, line: &mut [u8], acc: &mut [f32]) {
        let (p, pal, lut, cov, ch) = (self.p, self.pal, self.lut, self.cov, self.ch);
        let (lo, hi) = pixel_rows(p, y as u32, self.rows.len());
        if hi == lo {
            for px in line.as_chunks_mut::<4>().0 {
                px.copy_from_slice(&self.bg_px);
            }
            return;
        }
        acc.fill(0.0);
        let wf32 = self.w as f32;
        let mut n = 0u32;
        let (hi, k) = match p.mode {
            RasterMode::Sample => (lo + 1, 0),
            _ => (hi, p.max_samples),
        };
        for pos in sample_positions(lo, hi, k) {
            n += 1;
            let person = self.rows[pos] as usize;
            let s = &self.segs[self.starts[person] as usize..self.starts[person + 1] as usize];
            for (k, &seg) in s.iter().enumerate() {
                let t_e = s.get(k + 1).map_or(self.len16, |&nx| seg_start(nx));
                let xs = ((seg_start(seg) as f32 - self.t0) * self.scale).max(0.0);
                let xe = ((t_e as f32 - self.t0) * self.scale).min(wf32);
                if xe <= xs {
                    continue;
                }
                let code = seg_code(seg) as usize;
                match p.mode {
                    RasterMode::Mean => paint_rgb(acc, xs, xe, &pal.colors[code]),
                    RasterMode::Mode | RasterMode::Sample => paint(acc, ch, code, xs, xe),
                }
            }
        }
        let inv = 1.0 / n as f32;
        let bg = pal.background;
        match p.mode {
            RasterMode::Mean => {
                let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
                // Most columns contain no segment boundary: their colour equals
                // the previous column's, so skip the conversion for them.
                let mut last = [0u8; 4];
                let mut last_cov = f32::NAN;
                for ((px, a), &cv) in line
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(acc.as_chunks::<3>().0)
                    .zip(cov)
                {
                    if a[0] != 0.0 || a[1] != 0.0 || a[2] != 0.0 || cv != last_cov {
                        r += a[0];
                        g += a[1];
                        b += a[2];
                        let k = 1.0 - cv;
                        last = [
                            lut.get(r * inv + k * bg[0]),
                            lut.get(g * inv + k * bg[1]),
                            lut.get(b * inv + k * bg[2]),
                            255,
                        ];
                        last_cov = cv;
                    }
                    px.copy_from_slice(&last);
                }
            }
            RasterMode::Mode | RasterMode::Sample => {
                let mut run = [0.0f32; MAX_ACTIVITIES];
                let run = &mut run[..ch];
                for ((px, a), &cv) in line
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(acc.chunks_exact(ch))
                    .zip(cov)
                {
                    let (mut best, mut best_v) = (0usize, f32::NEG_INFINITY);
                    for (code, (r, &d)) in run.iter_mut().zip(a).enumerate() {
                        *r += d;
                        if *r > best_v + 1e-4 {
                            best_v = *r;
                            best = code;
                        }
                    }
                    let c = if cv < 0.5 { bg } else { pal.colors[best] };
                    px[0] = lut.get(c[0]);
                    px[1] = lut.get(c[1]);
                    px[2] = lut.get(c[2]);
                    px[3] = 255;
                }
            }
        }
    }
}

/// Weights of the (up to) four difference-array slots touched by a segment
/// covering pixels `[xs, xe)`: partial first pixel `is`, full pixels
/// `is+1..ie`, partial last pixel `ie`.
#[inline(always)]
fn weights(xs: f32, xe: f32) -> (usize, usize, [f32; 4]) {
    let is = xs as usize;
    let ie = xe as usize;
    if is == ie {
        let f = xe - xs;
        return (is, ie, [f, -f, 0.0, 0.0]);
    }
    let f0 = is as f32 + 1.0 - xs;
    let f1 = xe - ie as f32;
    (is, ie, [f0, 1.0 - f0, f1 - 1.0, -f1])
}

/// Adds colour `c` weighted by coverage of `[xs, xe)` to an RGB difference array.
#[inline(always)]
fn paint_rgb(acc: &mut [f32], xs: f32, xe: f32, c: &[f32; 3]) {
    let (is, ie, f) = weights(xs, xe);
    let mut add = |slot: usize, f: f32| {
        let a: &mut [f32; 3] = (&mut acc[slot * 3..slot * 3 + 3]).try_into().unwrap();
        a[0] += f * c[0];
        a[1] += f * c[1];
        a[2] += f * c[2];
    };
    add(is, f[0]);
    add(is + 1, f[1]);
    if ie != is {
        add(ie, f[2]);
        add(ie + 1, f[3]);
    }
}

/// Adds coverage of `[xs, xe)` to channel `code` of a `stride`-channel difference array.
#[inline(always)]
fn paint(acc: &mut [f32], stride: usize, code: usize, xs: f32, xe: f32) {
    let (is, ie, f) = weights(xs, xe);
    acc[is * stride + code] += f[0];
    acc[(is + 1) * stride + code] += f[1];
    if ie != is {
        acc[ie * stride + code] += f[2];
        acc[(ie + 1) * stride + code] += f[3];
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::needless_range_loop)]
    use super::*;
    use crate::testutil::dataset;
    use proptest::prelude::*;

    /// Schedules as (start, code) lists, plus the axis length.
    pub fn schedules_strategy() -> impl Strategy<Value = (u32, usize, Vec<Vec<(u16, u8)>>)> {
        (1u32..300, 1usize..6).prop_flat_map(|(len, n_act)| {
            let person = proptest::collection::btree_set(1u16..len.max(2) as u16, 0..8)
                .prop_flat_map(move |cuts| {
                    let cuts: Vec<u16> = cuts.into_iter().filter(|&c| (c as u32) < len).collect();
                    let n = cuts.len() + 1;
                    // Codes may repeat back to back, as the packer keeps such episodes.
                    proptest::collection::vec(0u8..n_act as u8, n).prop_map(move |codes| {
                        let starts = std::iter::once(0).chain(cuts.iter().copied());
                        starts.zip(codes).collect::<Vec<_>>()
                    })
                });
            (
                Just(len),
                Just(n_act),
                proptest::collection::vec(person, 1..40),
            )
        })
    }

    fn code_at(s: &[(u16, u8)], t: u32) -> u8 {
        s.iter().rev().find(|&&(st, _)| st as u32 <= t).unwrap().1
    }

    /// Brute force: per pixel, per sampled person, per minute (views are
    /// chosen so pixel edges fall on whole minutes or split minutes evenly).
    #[allow(clippy::too_many_arguments)]
    fn reference(
        len: u32,
        sched: &[Vec<(u16, u8)>],
        rows: &[u32],
        p: &RasterParams,
        pal: &Palette,
        n_act: usize,
    ) -> Vec<u8> {
        let (w, h) = (p.width as usize, p.height as usize);
        let mpp = (p.t1 - p.t0) / w as f64; // minutes per pixel
        let sub = if mpp >= 1.0 { mpp as usize } else { 1 };
        let mut out = vec![0u8; w * h * 4];
        let lut = SrgbLut::default();
        for y in 0..h {
            let (lo, hi) = pixel_rows(p, y as u32, rows.len());
            let people: Vec<usize> = if p.mode == RasterMode::Sample {
                rows[lo..hi].iter().take(1).map(|&r| r as usize).collect()
            } else {
                sample_positions(lo, hi, p.max_samples)
                    .map(|i| rows[i] as usize)
                    .collect()
            };
            for x in 0..w {
                let mut c = [0f32; 3];
                let mut counts = vec![0u32; n_act];
                let mut outside = 0u32;
                for k in 0..sub {
                    let t = p.t0 + x as f64 * mpp + (k as f64 + 0.5) * mpp / sub as f64;
                    if t < 0.0 || t >= len as f64 {
                        outside += 1;
                        continue;
                    }
                    for &q in &people {
                        let code = code_at(&sched[q], t as u32) as usize;
                        counts[code] += 1;
                        for ch in 0..3 {
                            c[ch] += pal.colors[code][ch] / (people.len() * sub) as f32;
                        }
                    }
                }
                let cov = 1.0 - outside as f32 / sub as f32;
                let col = if people.is_empty() {
                    pal.background
                } else if p.mode == RasterMode::Mean {
                    [0, 1, 2].map(|ch| c[ch] + (1.0 - cov) * pal.background[ch])
                } else if cov < 0.5 {
                    pal.background
                } else {
                    let best = (0..n_act)
                        .max_by_key(|&i| (counts[i], std::cmp::Reverse(i)))
                        .unwrap();
                    pal.colors[best]
                };
                let px = &mut out[(y * w + x) * 4..(y * w + x) * 4 + 4];
                px.copy_from_slice(&[lut.get(col[0]), lut.get(col[1]), lut.get(col[2]), 255]);
            }
        }
        out
    }

    fn palette() -> Palette {
        Palette::from_srgb(
            &[0xe69f00, 0x56b4e9, 0x009e73, 0xf0e442, 0x0072b2, 0xd55e00],
            0x202020,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]
        #[test]
        fn matches_brute_force(
            (len, n_act, sched) in schedules_strategy(),
            zoom in prop::sample::select(vec![-4i32, -2, -1, 1, 2, 4]),
            t0 in -20i32..300,
            width in 1u32..80,
            height in 1u32..30,
            ra in -3.0f64..45.0,
            rspan in 0.1f64..60.0,
            k in 0u32..5,
            mode in prop::sample::select(vec![RasterMode::Mean, RasterMode::Mode, RasterMode::Sample]),
            perm_seed in any::<u64>(),
        ) {
            let ds = dataset(len, n_act, &sched);
            let n = sched.len();
            // A shuffled subset of rows, like a filtered + sorted view.
            let mut rows: Vec<u32> = (0..n as u32).filter(|i| (perm_seed >> (i % 64)) & 1 == 1 || n < 3).collect();
            let r = (perm_seed as usize) % rows.len().max(1);
            rows.rotate_left(r);
            // zoom > 0: minutes per pixel; zoom < 0: pixels per minute.
            let span = if zoom > 0 { (width as i32 * zoom) as f64 } else { width as f64 / (-zoom) as f64 };
            let p = RasterParams {
                width, height,
                row_start: ra, row_end: ra + rspan,
                t0: t0 as f64, t1: t0 as f64 + span,
                max_samples: k,
                mode,
            };
            let pal = palette();
            let mut out = vec![0u8; (width * height * 4) as usize];
            rasterize(&ds, &rows, &p, &pal, &mut RasterScratch::default(), &mut out);
            let want = reference(len, &sched, &rows, &p, &pal, n_act);
            for (i, (a, b)) in out.iter().zip(&want).enumerate() {
                prop_assert!((*a as i32 - *b as i32).abs() <= 1, "pixel {} channel {}: {} vs {}", i / 4, i % 4, a, b);
            }
        }
    }

    #[test]
    fn bands_match_full_raster() {
        let sched: Vec<Vec<(u16, u8)>> = (0..300)
            .map(|i| vec![(0, 0), ((i % 97 + 1) as u16, 1), (150, (i % 3) as u8 + 2)])
            .collect();
        let ds = dataset(200, 5, &sched);
        let rows: Vec<u32> = (0..300).rev().collect();
        let p = RasterParams {
            width: 37,
            height: 23,
            row_start: 3.5,
            row_end: 290.0,
            t0: -10.0,
            t1: 190.0,
            max_samples: 4,
            mode: RasterMode::Mean,
        };
        let pal = palette();
        let mut full = vec![0u8; 37 * 23 * 4];
        rasterize(
            &ds,
            &rows,
            &p,
            &pal,
            &mut RasterScratch::default(),
            &mut full,
        );
        let mut banded = Vec::new();
        for y0 in (0..23).step_by(5) {
            let y1 = (y0 + 5).min(23);
            let mut b = vec![0u8; 37 * (y1 - y0) as usize * 4];
            rasterize_band(
                &ds,
                &rows,
                &p,
                &pal,
                &mut RasterScratch::default(),
                y0,
                y1,
                &mut b,
            );
            banded.extend(b);
        }
        assert_eq!(full, banded);
    }

    #[test]
    fn rows_and_samples() {
        let p = RasterParams {
            width: 1,
            height: 4,
            row_start: 0.0,
            row_end: 100.0,
            t0: 0.0,
            t1: 1.0,
            max_samples: 3,
            mode: RasterMode::Mean,
        };
        assert_eq!(pixel_rows(&p, 1, 100), (25, 50));
        assert_eq!(
            sample_positions(25, 50, 3).collect::<Vec<_>>(),
            vec![29, 37, 45]
        );
        assert_eq!(
            sample_positions(25, 27, 3).collect::<Vec<_>>(),
            vec![25, 26]
        );
        // Zoomed in: 2 rows over 8 pixels -> each pixel row shows one person.
        let p = RasterParams {
            row_end: 2.0,
            height: 8,
            ..p
        };
        let got: Vec<_> = (0..8).map(|y| pixel_rows(&p, y, 100)).collect();
        assert_eq!(got[0], (0, 1));
        assert_eq!(got[7], (1, 2));
    }
}
