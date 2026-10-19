//! One interface over CSV and Parquet tables: column names, then batches of
//! selected columns. CSV is streamed in chunks; Parquet is read one row
//! group at a time, so neither is ever held whole in memory.

use crate::source::ByteSource;
use crate::{Error, Result};
use bytes::Bytes;
use parquet::basic::{LogicalType, Type as PhysicalType};
use parquet::column::reader::ColumnReader;
use parquet::file::reader::{ChunkReader, FileReader, Length, SerializedFileReader};
use serde::Serialize;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};

/// Bytes read from a CSV source per call.
const CSV_CHUNK: usize = 4 << 20;
/// Rows per CSV batch.
pub const BATCH_ROWS: usize = 1 << 16;

/// Variable-length text values. Row `i` spans `data[ends[i-1]..ends[i]]`.
#[derive(Debug, Default, Clone)]
pub struct Text {
    pub data: Vec<u8>,
    pub ends: Vec<u32>,
    pub valid: Vec<bool>,
}

impl Text {
    pub fn push(&mut self, v: Option<&[u8]>) {
        if let Some(b) = v {
            self.data.extend_from_slice(b);
        }
        self.ends.push(self.data.len() as u32);
        self.valid.push(v.is_some());
    }

    #[inline]
    pub fn get(&self, i: usize) -> Option<&[u8]> {
        if !self.valid[i] {
            return None;
        }
        let a = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        Some(&self.data[a..self.ends[i] as usize])
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }
}

/// One column of a batch.
#[derive(Debug, Clone)]
pub enum ColData {
    /// Strings; every CSV column arrives as text.
    Text(Text),
    Int(Vec<i64>, Vec<bool>),
    /// NaN is missing.
    Float(Vec<f64>),
    /// 0, 1, or 255 for missing.
    Bool(Vec<u8>),
}

impl ColData {
    pub fn len(&self) -> usize {
        match self {
            ColData::Text(t) => t.len(),
            ColData::Int(v, _) => v.len(),
            ColData::Float(v) => v.len(),
            ColData::Bool(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The value as a number. Text is parsed; `Err(())` if it is not a number.
    #[inline]
    pub fn num(&self, i: usize) -> std::result::Result<Option<f64>, NotANumber> {
        Ok(match self {
            ColData::Text(t) => match t.get(i) {
                None => None,
                Some(b) => Some(parse_f64(b).ok_or(NotANumber)?),
            },
            ColData::Int(v, ok) => ok[i].then_some(v[i] as f64),
            ColData::Float(v) => (!v[i].is_nan()).then_some(v[i]),
            ColData::Bool(v) => (v[i] != 255).then_some(v[i] as f64),
        })
    }

    /// The value as text (numbers formatted), using `buf` for scratch.
    #[inline]
    pub fn key<'a>(&'a self, i: usize, buf: &'a mut Vec<u8>) -> Option<&'a [u8]> {
        use std::io::Write;
        match self {
            ColData::Text(t) => t.get(i),
            ColData::Int(v, ok) => {
                if !ok[i] {
                    return None;
                }
                write_int(buf, v[i]);
                Some(buf)
            }
            ColData::Float(v) => {
                if v[i].is_nan() {
                    return None;
                }
                buf.clear();
                write!(buf, "{:?}", v[i]).unwrap();
                Some(buf)
            }
            ColData::Bool(v) => match v[i] {
                0 => Some(b"false"),
                1 => Some(b"true"),
                _ => None,
            },
        }
    }
}

/// Formats an integer into `buf` (faster than `write!` in hot loops).
#[inline]
fn write_int(buf: &mut Vec<u8>, x: i64) {
    let mut tmp = [0u8; 20];
    let mut u = x.unsigned_abs();
    let mut k = tmp.len();
    loop {
        k -= 1;
        tmp[k] = b'0' + (u % 10) as u8;
        u /= 10;
        if u == 0 {
            break;
        }
    }
    buf.clear();
    if x < 0 {
        buf.push(b'-');
    }
    buf.extend_from_slice(&tmp[k..]);
}

/// A text value that should have been a number.
#[derive(Debug, Clone, Copy)]
pub struct NotANumber;

/// Receives batches and progress; returns false to stop.
pub type BatchFn<'a> = &'a mut dyn FnMut(&Batch, (u64, u64)) -> Result<bool>;
/// Receives CSV records (bytes, field ends, bytes read); returns false to stop.
type RecordFn<'a> = &'a mut dyn FnMut(&[u8], &[usize], u64) -> Result<bool>;

/// Parses a CSV number (surrounding spaces allowed).
#[inline]
pub fn parse_f64(b: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(b).ok()?.trim();
    if let Ok(i) = s.parse::<i64>() {
        return Some(i as f64);
    }
    s.parse::<f64>().ok()
}

/// A batch of rows of the selected columns.
#[derive(Debug, Clone)]
pub struct Batch {
    pub cols: Vec<ColData>,
    pub len: usize,
}

/// The type of a column as stored (Parquet) or as it looks (CSV: text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColType {
    Text,
    Int,
    Float,
    Bool,
    /// Nested or otherwise unreadable; skipped.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Parquet,
}

enum Kind {
    Csv {
        sep: u8,
    },
    Parquet {
        reader: Box<SerializedFileReader<Chunked>>,
        leaves: Vec<usize>,
        types: Vec<ColType>,
        decimal_scale: Vec<Option<i32>>,
    },
}

/// An opened table.
pub struct Table {
    src: Arc<dyn ByteSource>,
    kind: Kind,
    names: Vec<String>,
}

impl Table {
    /// Opens a table; Parquet is recognised by its magic bytes, anything else
    /// is CSV (separator from the extension or the first line).
    pub fn open(src: Arc<dyn ByteSource>, name: &str) -> Result<Table> {
        let mut magic = [0u8; 4];
        if src.len() >= 12 {
            src.read_at(0, &mut magic)?;
        }
        if &magic == b"PAR1" {
            return Table::open_parquet(src);
        }
        let sep = sniff_separator(&*src, name)?;
        let mut names = Vec::new();
        csv_records(&*src, sep, &mut |rec, ends, _| {
            names = fields(rec, ends)
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .collect();
            Ok(false)
        })?;
        if names.is_empty() {
            return Err(Error::new("the file is empty"));
        }
        check_unique(&names)?;
        Ok(Table {
            src,
            kind: Kind::Csv { sep },
            names,
        })
    }

    fn open_parquet(src: Arc<dyn ByteSource>) -> Result<Table> {
        let reader = SerializedFileReader::new(Chunked::new(src.clone()))?;
        let schema = reader.metadata().file_metadata().schema_descr_ptr();
        let (mut names, mut leaves, mut types, mut scales) = (vec![], vec![], vec![], vec![]);
        for (i, c) in schema.columns().iter().enumerate() {
            let path = c.path().parts();
            if path.len() != 1 || c.max_rep_level() > 0 {
                continue; // nested
            }
            let logical = c.logical_type_ref();
            let scale = match logical {
                Some(LogicalType::Decimal(d)) => Some(d.scale),
                _ => None,
            };
            let t = match c.physical_type() {
                PhysicalType::BOOLEAN => ColType::Bool,
                PhysicalType::INT32 | PhysicalType::INT64 if scale.is_some() => ColType::Float,
                PhysicalType::INT32 | PhysicalType::INT64 => ColType::Int,
                PhysicalType::FLOAT | PhysicalType::DOUBLE => ColType::Float,
                PhysicalType::BYTE_ARRAY => ColType::Text,
                _ => ColType::Unsupported,
            };
            names.push(path[0].clone());
            leaves.push(i);
            types.push(t);
            scales.push(scale);
        }
        check_unique(&names)?;
        Ok(Table {
            src,
            kind: Kind::Parquet {
                reader: Box::new(reader),
                leaves,
                types,
                decimal_scale: scales,
            },
            names,
        })
    }

    pub fn format(&self) -> Format {
        match self.kind {
            Kind::Csv { .. } => Format::Csv,
            Kind::Parquet { .. } => Format::Parquet,
        }
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Stored column types (CSV columns are all text until inferred).
    pub fn types(&self) -> Vec<ColType> {
        match &self.kind {
            Kind::Csv { .. } => vec![ColType::Text; self.names.len()],
            Kind::Parquet { types, .. } => types.clone(),
        }
    }

    pub fn column(&self, name: &str) -> Result<usize> {
        self.names.iter().position(|n| n == name).ok_or_else(|| {
            Error::new(format!(
                "no column {name:?} (columns: {})",
                self.names.join(", ")
            ))
        })
    }

    /// Row count if the format records it (Parquet).
    pub fn n_rows(&self) -> Option<u64> {
        match &self.kind {
            Kind::Csv { .. } => None,
            Kind::Parquet { reader, .. } => {
                Some(reader.metadata().file_metadata().num_rows() as u64)
            }
        }
    }

    /// The first `n` rows of every column (unsupported columns as missing text).
    pub fn head(&self, n: usize) -> Result<Batch> {
        let all: Vec<usize> = (0..self.names.len()).collect();
        let Kind::Parquet {
            reader,
            leaves,
            types,
            ..
        } = &self.kind
        else {
            let mut out = None;
            self.scan(&all, &mut |b, _| {
                let mut b = b.clone();
                for c in &mut b.cols {
                    if let ColData::Text(t) = c {
                        let k = n.min(t.len());
                        let end = if k == 0 { 0 } else { t.ends[k - 1] as usize };
                        t.data.truncate(end);
                        t.ends.truncate(k);
                        t.valid.truncate(k);
                    }
                }
                b.len = b.len.min(n);
                out = Some(b);
                Ok(false)
            })?;
            return Ok(out.unwrap_or(Batch {
                cols: all.iter().map(|_| ColData::Text(Text::default())).collect(),
                len: 0,
            }));
        };
        if reader.num_row_groups() == 0 {
            return Ok(Batch {
                cols: all.iter().map(|_| ColData::Text(Text::default())).collect(),
                len: 0,
            });
        }
        let rg = reader.get_row_group(0)?;
        let k = n.min(rg.metadata().num_rows() as usize);
        let mut cols = Vec::with_capacity(all.len());
        for c in all {
            if types[c] == ColType::Unsupported {
                let mut t = Text::default();
                (0..k).for_each(|_| t.push(None));
                cols.push(ColData::Text(t));
                continue;
            }
            let leaf = leaves[c];
            let max_def = rg.metadata().column(leaf).column_descr().max_def_level();
            cols.push(read_column(&mut rg.get_column_reader(leaf)?, max_def, k)?);
        }
        Ok(Batch { cols, len: k })
    }

    pub fn byte_len(&self) -> u64 {
        self.src.len()
    }

    /// Calls `f` with batches of the columns `cols` (indices into `names`)
    /// and progress `(done, total)` in bytes (CSV) or rows (Parquet).
    /// `f` returns false to stop early.
    pub fn scan(&self, cols: &[usize], f: BatchFn) -> Result<()> {
        match &self.kind {
            Kind::Csv { sep } => self.scan_csv(*sep, cols, f),
            Kind::Parquet {
                reader,
                leaves,
                types,
                decimal_scale,
            } => {
                for &c in cols {
                    if types[c] == ColType::Unsupported {
                        return Err(Error::new(format!(
                            "column {:?} has an unsupported type",
                            self.names[c]
                        )));
                    }
                }
                let total = reader.metadata().file_metadata().num_rows() as u64;
                let mut done = 0;
                for g in 0..reader.num_row_groups() {
                    // Read each row group in batches so a huge row group never
                    // has to be decoded at once.
                    let rg = reader.get_row_group(g)?;
                    let mut left = rg.metadata().num_rows() as usize;
                    let mut readers = Vec::with_capacity(cols.len());
                    for &c in cols {
                        let leaf = leaves[c];
                        let max_def = rg.metadata().column(leaf).column_descr().max_def_level();
                        readers.push((rg.get_column_reader(leaf)?, max_def, decimal_scale[c]));
                    }
                    while left > 0 {
                        let n = left.min(BATCH_ROWS);
                        left -= n;
                        let mut out = Vec::with_capacity(cols.len());
                        for (r, max_def, scale) in &mut readers {
                            let col = read_column(r, *max_def, n)?;
                            if col.len() != n {
                                return Err(Error::new(
                                    "Parquet column chunk is shorter than its row group",
                                ));
                            }
                            out.push(match (col, *scale) {
                                (ColData::Int(v, ok), Some(s)) => ColData::Float(
                                    v.iter()
                                        .zip(&ok)
                                        .map(|(&x, &k)| {
                                            if k {
                                                x as f64 / 10f64.powi(s)
                                            } else {
                                                f64::NAN
                                            }
                                        })
                                        .collect(),
                                ),
                                (col, _) => col,
                            });
                        }
                        done += n as u64;
                        if !f(&Batch { cols: out, len: n }, (done, total))? {
                            return Ok(());
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn scan_csv(&self, sep: u8, cols: &[usize], f: BatchFn) -> Result<()> {
        let ncols = self.names.len();
        let total = self.src.len();
        let empty = || Batch {
            cols: cols
                .iter()
                .map(|_| ColData::Text(Text::default()))
                .collect(),
            len: 0,
        };
        let mut batch = empty();
        let mut row = 0u64;
        let mut first = true;
        let mut stopped = false;
        csv_records(&*self.src, sep, &mut |rec, ends, pos| {
            if std::mem::take(&mut first) {
                return Ok(true); // header
            }
            row += 1;
            if ends.len() != ncols {
                return Err(Error::new(format!(
                    "data row {row} has {} fields, the header has {ncols}",
                    ends.len()
                )));
            }
            for (col, &c) in batch.cols.iter_mut().zip(cols) {
                let a = if c == 0 { 0 } else { ends[c - 1] };
                let v = &rec[a..ends[c]];
                let ColData::Text(t) = col else {
                    unreachable!()
                };
                t.push((!v.is_empty()).then_some(v));
            }
            batch.len += 1;
            if batch.len == BATCH_ROWS {
                let full = std::mem::replace(&mut batch, empty());
                if !f(&full, (pos, total))? {
                    stopped = true;
                    return Ok(false);
                }
            }
            Ok(true)
        })?;
        if !stopped && batch.len > 0 {
            f(&batch, (total, total))?;
        }
        Ok(())
    }
}

fn check_unique(names: &[String]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for n in names {
        if !seen.insert(n) {
            return Err(Error::new(format!("duplicate column name {n:?}")));
        }
    }
    Ok(())
}

/// The fields of a csv-core record.
fn fields<'a>(rec: &'a [u8], ends: &'a [usize]) -> impl Iterator<Item = &'a [u8]> + 'a {
    ends.iter().enumerate().map(move |(i, &e)| {
        let a = if i == 0 { 0 } else { ends[i - 1] };
        &rec[a..e]
    })
}

fn sniff_separator(src: &dyn ByteSource, name: &str) -> Result<u8> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tsv") || lower.ends_with(".tab") {
        return Ok(b'\t');
    }
    let mut buf = vec![0u8; src.len().min(64 << 10) as usize];
    src.read_at(0, &mut buf)?;
    let line = buf.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let mut best = (0, b',');
    for sep in *b",\t;|" {
        let mut n = 0;
        let mut quoted = false;
        for &b in line {
            if b == b'"' {
                quoted = !quoted;
            } else if b == sep && !quoted {
                n += 1;
            }
        }
        if n > best.0 {
            best = (n, sep);
        }
    }
    Ok(best.1)
}

/// Streams every record (the header first) to `f(record, field ends, bytes read)`.
/// `f` returns false to stop.
fn csv_records(src: &dyn ByteSource, sep: u8, f: RecordFn) -> Result<()> {
    use csv_core::ReadRecordResult as R;
    let mut rdr = csv_core::ReaderBuilder::new().delimiter(sep).build();
    let total = src.len();
    let mut input = vec![0u8; CSV_CHUNK.min(total as usize)];
    let (mut file_pos, mut in_len, mut in_pos) = (0u64, 0usize, 0usize);
    let mut out = vec![0u8; 1 << 12];
    let mut ends = vec![0usize; 64];
    let (mut nout, mut nend) = (0, 0);
    loop {
        if in_pos == in_len && file_pos < total {
            in_len = (total - file_pos).min(input.len() as u64) as usize;
            src.read_at(file_pos, &mut input[..in_len])?;
            file_pos += in_len as u64;
            in_pos = 0;
        }
        let (res, nin, o, e) =
            rdr.read_record(&input[in_pos..in_len], &mut out[nout..], &mut ends[nend..]);
        in_pos += nin;
        nout += o;
        nend += e;
        match res {
            R::InputEmpty => {}
            R::OutputFull => out.resize(out.len() * 2, 0),
            R::OutputEndsFull => ends.resize(ends.len() * 2, 0),
            R::Record => {
                let read = file_pos - (in_len - in_pos) as u64;
                if !f(&out[..nout], &ends[..nend], read)? {
                    return Ok(());
                }
                nout = 0;
                nend = 0;
            }
            R::End => return Ok(()),
        }
    }
}

/// Reads the next `n` rows (fewer at the end) of a column chunk.
fn read_column(reader: &mut ColumnReader, max_def: i16, n: usize) -> Result<ColData> {
    macro_rules! read_all {
        ($r:expr) => {{
            let r = $r;
            let mut vals = Vec::with_capacity(n);
            let mut defs = Vec::with_capacity(if max_def > 0 { n } else { 0 });
            let mut left = n;
            while left > 0 {
                let d = if max_def > 0 { Some(&mut defs) } else { None };
                let (recs, _, _) = r.read_records(left, d, None, &mut vals)?;
                if recs == 0 {
                    break;
                }
                left -= recs;
            }
            (vals, defs)
        }};
    }
    // Spreads compacted non-null values over the rows using definition levels.
    fn spread<T: Copy>(vals: Vec<T>, defs: &[i16], max_def: i16, null: T) -> (Vec<T>, Vec<bool>) {
        if max_def == 0 {
            let ok = vec![true; vals.len()];
            return (vals, ok);
        }
        let mut it = vals.into_iter();
        let mut out = Vec::with_capacity(defs.len());
        let mut ok = Vec::with_capacity(defs.len());
        for &d in defs {
            if d == max_def {
                out.push(it.next().unwrap_or(null));
                ok.push(true);
            } else {
                out.push(null);
                ok.push(false);
            }
        }
        (out, ok)
    }
    Ok(match reader {
        ColumnReader::BoolColumnReader(r) => {
            let (v, d) = read_all!(r);
            let (v, ok) = spread(v, &d, max_def, false);
            ColData::Bool(
                v.iter()
                    .zip(&ok)
                    .map(|(&x, &k)| if k { x as u8 } else { 255 })
                    .collect(),
            )
        }
        ColumnReader::Int32ColumnReader(r) => {
            let (v, d) = read_all!(r);
            let (v, ok) = spread(v, &d, max_def, 0);
            ColData::Int(v.into_iter().map(|x| x as i64).collect(), ok)
        }
        ColumnReader::Int64ColumnReader(r) => {
            let (v, d) = read_all!(r);
            let (v, ok) = spread(v, &d, max_def, 0);
            ColData::Int(v, ok)
        }
        ColumnReader::FloatColumnReader(r) => {
            let (v, d) = read_all!(r);
            let (v, _) = spread(v, &d, max_def, f32::NAN);
            ColData::Float(v.into_iter().map(|x| x as f64).collect())
        }
        ColumnReader::DoubleColumnReader(r) => {
            let (v, d) = read_all!(r);
            ColData::Float(spread(v, &d, max_def, f64::NAN).0)
        }
        ColumnReader::ByteArrayColumnReader(r) => {
            let (v, d) = read_all!(r);
            let mut t = Text::default();
            t.data.reserve(v.iter().map(|b| b.len()).sum());
            if max_def == 0 {
                v.iter().for_each(|b| t.push(Some(b.data())));
            } else {
                let mut it = v.iter();
                for &x in &d {
                    t.push(if x == max_def {
                        it.next().map(|b| b.data())
                    } else {
                        None
                    });
                }
            }
            ColData::Text(t)
        }
        _ => return Err(Error::new("unsupported Parquet column type")),
    })
}

/// A `ChunkReader` over a `ByteSource`, with a read-ahead window so the many
/// small reads of page headers don't each hit the host.
struct Chunked {
    src: Arc<dyn ByteSource>,
    window: Mutex<(u64, Bytes)>,
}

const WINDOW: u64 = 8 << 20;

impl Chunked {
    fn new(src: Arc<dyn ByteSource>) -> Chunked {
        Chunked {
            src,
            window: Mutex::new((0, Bytes::new())),
        }
    }

    fn bytes(&self, start: u64, len: usize) -> io::Result<Bytes> {
        let end = start + len as u64;
        if end > self.src.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "read past end",
            ));
        }
        let mut w = self.window.lock().unwrap();
        if start < w.0 || end > w.0 + w.1.len() as u64 {
            let wend = (start + (len as u64).max(WINDOW)).min(self.src.len());
            let mut buf = vec![0u8; (wend - start) as usize];
            self.src.read_at(start, &mut buf)?;
            *w = (start, Bytes::from(buf));
        }
        let a = (start - w.0) as usize;
        Ok(w.1.slice(a..a + len))
    }
}

impl Length for Chunked {
    fn len(&self) -> u64 {
        self.src.len()
    }
}

/// Sequential reads from a `Chunked`, starting at an offset.
struct ChunkedRead {
    inner: Arc<Chunked>,
    pos: u64,
}

impl Read for ChunkedRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (self.inner.src.len().saturating_sub(self.pos)).min(buf.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        let b = self.inner.bytes(self.pos, n)?;
        buf[..n].copy_from_slice(&b);
        self.pos += n as u64;
        Ok(n)
    }
}

impl ChunkReader for Chunked {
    type T = ChunkedRead;

    fn get_read(&self, start: u64) -> parquet::errors::Result<ChunkedRead> {
        // A cheap clone that shares the window.
        let inner = Arc::new(Chunked {
            src: self.src.clone(),
            window: Mutex::new(self.window.lock().unwrap().clone()),
        });
        Ok(ChunkedRead { inner, pos: start })
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<Bytes> {
        Ok(self.bytes(start, length)?)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn write_int_matches_display() {
        let mut b = Vec::new();
        for x in [0, 7, -7, 10, 1234567890, i64::MIN, i64::MAX] {
            super::write_int(&mut b, x);
            assert_eq!(b, x.to_string().as_bytes());
        }
    }
}
