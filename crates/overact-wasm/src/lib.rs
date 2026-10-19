//! Thin wasm-bindgen API over `overact-core`.
//!
//! Large data never crosses as JS objects: the host gets pointers into WASM
//! memory (file slices are copied straight into dataset buffers; pixel and
//! profile buffers are read in place). Everything else is small JSON.

use overact_core::dataset::Part;
use overact_core::engine::Engine;
use overact_core::filter::ColRef;
use overact_core::raster::{Palette, RasterMode, RasterParams};
use overact_core::{Dataset, Header, ViewSpec};
use overact_ingest::{ByteSource, Table};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[cfg(feature = "parallel")]
pub use wasm_bindgen_rayon::init_thread_pool;

/// Number of threads the engine uses (1 in the single-threaded build).
#[wasm_bindgen]
pub fn thread_count() -> usize {
    #[cfg(feature = "parallel")]
    {
        rayon::current_num_threads()
    }
    #[cfg(not(feature = "parallel"))]
    {
        1
    }
}

fn err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

fn part(kind: u32, column: u32) -> Result<Part, JsError> {
    Ok(match kind {
        0 => Part::Offsets,
        1 => Part::Segments,
        2 => Part::PersonIdOffsets,
        3 => Part::PersonIdData,
        4 => Part::Column(column as usize),
        _ => return Err(JsError::new("bad part kind")),
    })
}

/// A `File`/`Blob` read synchronously through a host function
/// `read(blob, offset, length) -> Uint8Array` (`FileReaderSync` in a worker).
struct JsSource {
    blob: JsValue,
    read: js_sys::Function,
    len: u64,
}

// SAFETY: a JsSource is created and only ever read on the worker's main
// thread (ingest does not read from rayon threads). The Parquet reader
// requires the bounds.
unsafe impl Send for JsSource {}
unsafe impl Sync for JsSource {}

impl ByteSource for JsSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let fail = |m: String| std::io::Error::other(m);
        let out = self
            .read
            .call3(
                &JsValue::NULL,
                &self.blob,
                &JsValue::from_f64(offset as f64),
                &JsValue::from_f64(buf.len() as f64),
            )
            .map_err(|e| fail(format!("{e:?}")))?;
        let arr = js_sys::Uint8Array::new(&out);
        if arr.length() as usize != buf.len() {
            return Err(fail("short read".into()));
        }
        arr.copy_to(buf);
        Ok(())
    }
}

fn open_table(
    blob: JsValue,
    size: f64,
    name: &str,
    read: &js_sys::Function,
) -> Result<Table, JsError> {
    let src: Arc<dyn ByteSource> = Arc::new(JsSource {
        blob,
        read: read.clone(),
        len: size as u64,
    });
    Table::open(src, name).map_err(err)
}

/// Column names, types and the first rows of a CSV/Parquet table, as JSON.
#[wasm_bindgen]
pub fn inspect_table(
    blob: JsValue,
    size: f64,
    name: &str,
    read: &js_sys::Function,
) -> Result<String, JsError> {
    let t = open_table(blob, size, name, read)?;
    let info = overact_ingest::inspect(&t, 5).map_err(err)?;
    serde_json::to_string(&info).map_err(err)
}

fn default_palette() -> Palette {
    let colors: Vec<u32> = (0..32).map(|i| 0x101010 * (i % 16)).collect();
    Palette::from_srgb(&colors, 0xffffff)
}

#[wasm_bindgen]
pub struct WasmEngine {
    engine: Engine,
    palette: Palette,
    raster_buf: Vec<u8>,
}

#[wasm_bindgen]
impl WasmEngine {
    /// Creates an engine for a file's JSON header and allocates schedule buffers.
    #[wasm_bindgen(constructor)]
    pub fn new(header_json: &str) -> Result<WasmEngine, JsError> {
        let header = Header::parse(header_json.as_bytes()).map_err(err)?;
        Ok(WasmEngine::with(Dataset::new(header)))
    }

    fn with(ds: Dataset) -> WasmEngine {
        WasmEngine {
            engine: Engine::new(ds),
            palette: default_palette(),
            raster_buf: Vec::new(),
        }
    }

    /// Builds the dataset from an activities table and an optional attributes
    /// table (pass null). `config_json` is an `overact_ingest::Config`;
    /// `progress(phase, done, total)` is called as reading proceeds.
    #[allow(clippy::too_many_arguments)]
    pub fn from_tables(
        acts: JsValue,
        acts_size: f64,
        acts_name: &str,
        attrs: JsValue,
        attrs_size: f64,
        attrs_name: &str,
        read: &js_sys::Function,
        config_json: &str,
        progress: &js_sys::Function,
    ) -> Result<WasmEngine, JsError> {
        let cfg: overact_ingest::Config = serde_json::from_str(config_json).map_err(err)?;
        let acts = open_table(acts, acts_size, acts_name, read)?;
        let attrs = if attrs.is_null() || attrs.is_undefined() {
            None
        } else {
            Some(open_table(attrs, attrs_size, attrs_name, read)?)
        };
        let mut report = |phase: &str, done: u64, total: u64| {
            let _ = progress.call3(
                &JsValue::NULL,
                &JsValue::from_str(phase),
                &JsValue::from_f64(done as f64),
                &JsValue::from_f64(total as f64),
            );
        };
        let ds = overact_ingest::ingest(&acts, attrs.as_ref(), &cfg, &mut report).map_err(err)?;
        Ok(WasmEngine::with(ds))
    }

    /// The engine's header as JSON (blob offsets as a written file would have them).
    pub fn header_json(&self) -> String {
        serde_json::to_string(&self.engine.ds.header).unwrap()
    }

    /// The header for writing an `.overact` file (offsets laid out).
    pub fn file_header(&self) -> String {
        let mut h = self.engine.ds.header.clone();
        String::from_utf8(h.layout()).unwrap()
    }

    /// Pointer to a loaded part's bytes without changing it (0 if not loaded).
    pub fn part_view_ptr(&self, kind: u32, column: u32) -> Result<usize, JsError> {
        let p = part(kind, column)?;
        Ok(self
            .engine
            .ds
            .part_bytes(p)
            .map_or(0, |b| b.as_ptr() as usize))
    }

    pub fn part_view_len(&self, kind: u32, column: u32) -> Result<usize, JsError> {
        let p = part(kind, column)?;
        Ok(self.engine.ds.part_bytes(p).map_or(0, |b| b.len()))
    }

    /// Pointer to the buffer for a file part (allocating it if needed).
    /// kind: 0 offsets, 1 segments, 2 person id offsets, 3 person id data, 4 column.
    pub fn part_ptr(&mut self, kind: u32, column: u32) -> Result<usize, JsError> {
        let p = part(kind, column)?;
        Ok(self.engine.ds.part_bytes_mut(p).map_err(err)?.as_mut_ptr() as usize)
    }

    pub fn part_len(&mut self, kind: u32, column: u32) -> Result<usize, JsError> {
        let p = part(kind, column)?;
        Ok(self.engine.ds.part_bytes_mut(p).map_err(err)?.len())
    }

    /// File byte offset of a part.
    pub fn part_offset(&self, kind: u32, column: u32) -> Result<f64, JsError> {
        Ok(self
            .engine
            .ds
            .blob(part(kind, column)?)
            .map_err(err)?
            .byte_offset as f64)
    }

    pub fn finish_schedules(&mut self) -> Result<(), JsError> {
        self.engine.ds.finish_schedules().map_err(err)
    }

    pub fn finish_column(&mut self, column: u32) -> Result<(), JsError> {
        self.engine.ds.finish_column(column as usize).map_err(err)
    }

    pub fn column_loaded(&self, column: u32) -> bool {
        self.engine.ds.column_loaded(column as usize)
    }

    /// Attribute columns that must be loaded before `spec_json` can run.
    pub fn missing_columns(&self, spec_json: &str) -> Result<Vec<u32>, JsError> {
        let spec: ViewSpec = serde_json::from_str(spec_json).map_err(err)?;
        Ok(self
            .engine
            .missing_columns(&spec)
            .into_iter()
            .map(|c| c as u32)
            .collect())
    }

    /// Queues a view job; false if the view already shows this spec.
    pub fn submit(&mut self, view: u32, spec_json: &str) -> Result<bool, JsError> {
        let spec: ViewSpec = serde_json::from_str(spec_json).map_err(err)?;
        self.engine.submit(view, spec).map_err(err)
    }

    pub fn drop_view(&mut self, view: u32) {
        self.engine.drop_view(view);
    }

    pub fn has_work(&self) -> bool {
        self.engine.has_work()
    }

    /// Runs about `budget` element operations; returns StepResult JSON.
    pub fn step(&mut self, budget: u32) -> String {
        serde_json::to_string(&self.engine.step(budget as usize)).unwrap()
    }

    /// `{count, matched, version}` of a view (rows shown, persons passing the
    /// filter before sampling), or null.
    pub fn view_info(&self, view: u32) -> String {
        match self.engine.view(view) {
            Some(v) => format!(
                r#"{{"count":{},"matched":{},"version":{}}}"#,
                v.rows.len(),
                v.filtered.len(),
                v.version
            ),
            None => "null".into(),
        }
    }

    /// Sets activity colours (sRGB 0xRRGGBB per code) and the background.
    pub fn set_palette(&mut self, colors: Vec<u32>, background: u32) {
        self.palette = Palette::from_srgb(&colors, background);
    }

    /// Draws pixel rows `y0..y1` of a view into an internal RGBA buffer and
    /// returns its pointer (0 if the view is not ready). mode: 0 mean, 1 mode, 2 sample (first row only).
    #[allow(clippy::too_many_arguments)]
    pub fn raster(
        &mut self,
        view: u32,
        width: u32,
        height: u32,
        row_start: f64,
        row_end: f64,
        t0: f64,
        t1: f64,
        max_samples: u32,
        mode: u32,
        y0: u32,
        y1: u32,
    ) -> usize {
        let p = RasterParams {
            width,
            height,
            row_start,
            row_end,
            t0,
            t1,
            max_samples,
            mode: match mode {
                1 => RasterMode::Mode,
                2 => RasterMode::Sample,
                _ => RasterMode::Mean,
            },
        };
        // A panic would leave the engine unusable: reject degenerate input.
        let finite = [row_start, row_end, t0, t1].iter().all(|x| x.is_finite());
        if width == 0 || height == 0 || !finite || t1 <= t0 || row_end < row_start {
            return 0;
        }
        let (y0, y1) = (y0.min(height), y1.min(height).max(y0.min(height)));
        self.raster_buf
            .resize(width as usize * (y1 - y0) as usize * 4, 0);
        if self
            .engine
            .raster_band(view, &p, &self.palette, y0, y1, &mut self.raster_buf)
        {
            self.raster_buf.as_ptr() as usize
        } else {
            0
        }
    }

    /// A view's profile (u32 `[code * axis_len + minute]`), if computed.
    pub fn profile(&self, view: u32) -> Option<Vec<u32>> {
        let v = self.engine.view(view)?;
        v.profile.as_ref().map(|p| p.to_vec())
    }

    /// A view's person ids in row order. `csv`: a `person_id,row` CSV file;
    /// otherwise UTF-8 strings as `u32 offsets[count + 1]` then the bytes.
    pub fn export_ids(&self, view: u32, csv: bool) -> Result<Vec<u8>, JsError> {
        let v = self
            .engine
            .view(view)
            .ok_or_else(|| JsError::new("view not ready"))?;
        let ds = &self.engine.ds;
        if csv {
            return Ok(overact_core::export::ids_csv(ds, &v.rows));
        }
        let (offsets, data) = overact_core::export::ids_utf8(ds, &v.rows);
        let mut out = Vec::with_capacity(offsets.len() * 4 + data.len());
        for o in offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&data);
        Ok(out)
    }

    /// Total minutes in each activity over all persons (by code).
    pub fn activity_minutes(&self) -> Vec<f64> {
        self.engine
            .ds
            .activity_minutes()
            .into_iter()
            .map(|m| m as f64)
            .collect()
    }

    /// Value of a column for each pixel row's middle person (NaN if missing).
    #[allow(clippy::too_many_arguments)]
    pub fn gutter(
        &self,
        view: u32,
        col_json: &str,
        height: u32,
        row_start: f64,
        row_end: f64,
    ) -> Result<Vec<f64>, JsError> {
        let col: ColRef = serde_json::from_str(col_json).map_err(err)?;
        let p = RasterParams {
            width: 1,
            height,
            row_start,
            row_end,
            t0: 0.0,
            t1: 1.0,
            max_samples: 1,
            mode: RasterMode::Mean,
        };
        self.engine.gutter(view, &col, &p).map_err(err)
    }

    /// Person index at a row position of a view, or -1.
    pub fn person_at(&self, view: u32, pos: f64) -> f64 {
        if pos.is_nan() || pos < 0.0 {
            return -1.0;
        }
        self.engine
            .person_at(view, pos as usize)
            .map_or(-1.0, |p| p as f64)
    }

    /// `{id, segments: [[start, end, code], ...]}` for person `p`.
    pub fn person_info(&self, p: u32) -> String {
        let ds = &self.engine.ds;
        let p = p as usize;
        if p >= ds.n_persons() {
            return "null".into();
        }
        let segs: Vec<[u16; 3]> = ds
            .person_segments(p)
            .map(|(s, e, c)| [s, e, c as u16])
            .collect();
        serde_json::json!({ "index": p, "id": ds.person_id(p), "segments": segs }).to_string()
    }

    /// Value of a column for person `p` (NaN if missing or unavailable).
    pub fn value(&self, col_json: &str, p: u32) -> f64 {
        let Ok(col) = serde_json::from_str::<ColRef>(col_json) else {
            return f64::NAN;
        };
        match self.engine.column(&col) {
            Ok(c) if (p as usize) < c.len() => c.get(p as usize).unwrap_or(f64::NAN),
            _ => f64::NAN,
        }
    }

    /// `{min, max}` of a column (attribute or computed feature), or null.
    pub fn column_range(&self, col_json: &str) -> String {
        let Ok(col) = serde_json::from_str::<ColRef>(col_json) else {
            return "null".into();
        };
        match self.engine.column(&col) {
            Ok(c) => serde_json::json!({"min": c.meta.min, "max": c.meta.max}).to_string(),
            Err(_) => "null".into(),
        }
    }

    /// Current WASM memory size in bytes.
    pub fn memory_bytes(&self) -> f64 {
        #[cfg(target_arch = "wasm32")]
        {
            (core::arch::wasm32::memory_size(0) * 65536) as f64
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            0.0
        }
    }
}
