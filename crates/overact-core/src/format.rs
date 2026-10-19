//! `.overact` header parsing. See `docs/FORMAT.md`.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAGIC: &[u8; 8] = b"OVERACT1";
/// Bytes before the JSON header: magic plus u32 length.
pub const PRELUDE_LEN: usize = 12;
pub const MAX_AXIS_LEN: u32 = 2048;
pub const MAX_ACTIVITIES: usize = 32;
pub const START_SHIFT: u16 = 5;
pub const CODE_MASK: u16 = 0x1f;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    BadMagic,
    Truncated,
    Header(String),
    Invalid(String),
    UnknownColumn(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadMagic => write!(f, "not an .overact file (bad magic)"),
            Error::Truncated => write!(f, "file is truncated"),
            Error::Header(e) => write!(f, "bad header: {e}"),
            Error::Invalid(e) => write!(f, "invalid file: {e}"),
            Error::UnknownColumn(c) => write!(f, "unknown column {c:?}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    U8,
    U16,
    U32,
    I8,
    I16,
    I32,
    F32,
    F64,
    Utf8,
}

impl DType {
    pub fn size(self) -> usize {
        match self {
            DType::U8 | DType::I8 | DType::Utf8 => 1,
            DType::U16 | DType::I16 => 2,
            DType::U32 | DType::I32 | DType::F32 => 4,
            DType::F64 => 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnKind {
    Numeric,
    Categorical,
    Boolean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobRef {
    pub dtype: DType,
    pub byte_offset: u64,
    pub byte_length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeAxis {
    pub start: i32,
    pub end: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl TimeAxis {
    /// `end - start`, or 0 if `end <= start` (no overflow for extreme values).
    pub fn len(&self) -> u32 {
        (self.end as i64 - self.start as i64).clamp(0, u32::MAX as i64) as u32
    }
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub code: u8,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blobs {
    pub offsets: BlobRef,
    pub segments: BlobRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id_offsets: Option<BlobRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id_data: Option<BlobRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    pub name: String,
    pub kind: ColumnKind,
    pub dtype: DType,
    pub byte_offset: u64,
    pub byte_length: u64,
    pub null_value: Option<f64>,
    #[serde(default)]
    pub null_count: Option<u64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
}

impl ColumnMeta {
    pub fn blob(&self) -> BlobRef {
        BlobRef {
            dtype: self.dtype,
            byte_offset: self.byte_offset,
            byte_length: self.byte_length,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub version: u32,
    pub n_persons: u32,
    pub n_segments: u32,
    pub time_axis: TimeAxis,
    pub activities: Vec<Activity>,
    pub blobs: Blobs,
    pub columns: Vec<ColumnMeta>,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// Reads the header length from the first 12 bytes of a file.
pub fn header_len(prelude: &[u8]) -> Result<usize> {
    if prelude.len() < PRELUDE_LEN {
        return Err(Error::Truncated);
    }
    if &prelude[..8] != MAGIC {
        return Err(Error::BadMagic);
    }
    Ok(u32::from_le_bytes(prelude[8..12].try_into().unwrap()) as usize)
}

impl Header {
    /// Parses and checks the JSON header (the bytes after the 12-byte prelude).
    pub fn parse(json: &[u8]) -> Result<Header> {
        let h: Header = serde_json::from_slice(json).map_err(|e| Error::Header(e.to_string()))?;
        h.check()?;
        Ok(h)
    }

    /// Parses a header from the start of a file (prelude + JSON).
    pub fn from_file_start(bytes: &[u8]) -> Result<Header> {
        let n = header_len(bytes)?;
        let json = bytes
            .get(PRELUDE_LEN..PRELUDE_LEN + n)
            .ok_or(Error::Truncated)?;
        Header::parse(json)
    }

    pub(crate) fn check(&self) -> Result<()> {
        let bad = |s: String| Err(Error::Invalid(s));
        if self.format != "overact" || self.version != 1 {
            return bad(format!(
                "unsupported format {} v{}",
                self.format, self.version
            ));
        }
        if self.time_axis.is_empty() || self.time_axis.len() > MAX_AXIS_LEN {
            return bad(format!(
                "time axis length {} out of range",
                self.time_axis.len()
            ));
        }
        if self.activities.len() > MAX_ACTIVITIES {
            return bad(format!(
                "{} activities (max {MAX_ACTIVITIES})",
                self.activities.len()
            ));
        }
        for (i, a) in self.activities.iter().enumerate() {
            if a.code as usize != i {
                return bad("activity codes must be 0..n in order".into());
            }
        }
        let n = self.n_persons as u64;
        let expect = |b: &BlobRef, dt: DType, len: u64, what: &str| {
            if b.dtype != dt
                || b.byte_length != len * dt.size() as u64
                || !b.byte_offset.is_multiple_of(8)
            {
                Err(Error::Invalid(format!(
                    "{what}: wrong dtype, length or alignment"
                )))
            } else {
                Ok(())
            }
        };
        expect(&self.blobs.offsets, DType::U32, n + 1, "offsets")?;
        expect(
            &self.blobs.segments,
            DType::U16,
            self.n_segments as u64,
            "segments",
        )?;
        if let Some(b) = &self.blobs.person_id_offsets {
            expect(b, DType::U32, n + 1, "person_id_offsets")?;
            if self.blobs.person_id_data.is_none() {
                return bad("person_id_offsets without person_id_data".into());
            }
        }
        for c in &self.columns {
            let ok = match c.kind {
                ColumnKind::Numeric => {
                    matches!(
                        c.dtype,
                        DType::F32 | DType::F64 | DType::I8 | DType::I16 | DType::I32
                    )
                }
                ColumnKind::Categorical => matches!(c.dtype, DType::U8 | DType::U16),
                ColumnKind::Boolean => c.dtype == DType::U8,
            };
            if !ok {
                return bad(format!(
                    "column {:?}: dtype not allowed for its kind",
                    c.name
                ));
            }
            expect(&c.blob(), c.dtype, n, &format!("column {:?}", c.name))?;
            // Readers rely on the spec's sentinels (e.g. sort keys of signed
            // integers assume the null is the type minimum).
            let null = match c.dtype {
                DType::I8 => Some(i8::MIN as f64),
                DType::I16 => Some(i16::MIN as f64),
                DType::I32 => Some(i32::MIN as f64),
                DType::U8 => Some(u8::MAX as f64),
                DType::U16 => Some(u16::MAX as f64),
                _ => None,
            };
            if c.null_value != null {
                return bad(format!(
                    "column {:?}: nullValue must be {}",
                    c.name,
                    null.map_or("null".into(), |v| v.to_string())
                ));
            }
        }
        Ok(())
    }

    pub fn column_index(&self, name: &str) -> Result<usize> {
        self.columns
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| Error::UnknownColumn(name.into()))
    }

    pub fn axis_len(&self) -> u32 {
        self.time_axis.len()
    }

    /// Assigns every blob the byte offset a writer gives it (blobs in the
    /// order offsets, segments, person ids, columns, each 8-byte aligned
    /// after the header) and returns the JSON header bytes to write.
    pub fn layout(&mut self) -> Vec<u8> {
        // Offsets depend on the header length, which depends on the offsets'
        // digit count: iterate until stable (two or three rounds).
        let mut json = Vec::new();
        loop {
            let mut pos = align8((PRELUDE_LEN + json.len()) as u64);
            let mut place = |b: &mut BlobRef| {
                b.byte_offset = pos;
                pos = align8(pos + b.byte_length);
            };
            place(&mut self.blobs.offsets);
            place(&mut self.blobs.segments);
            if let Some(b) = self.blobs.person_id_offsets.as_mut() {
                place(b);
            }
            if let Some(b) = self.blobs.person_id_data.as_mut() {
                place(b);
            }
            for c in &mut self.columns {
                let mut b = c.blob();
                place(&mut b);
                c.byte_offset = b.byte_offset;
            }
            let new = serde_json::to_vec(self).expect("header serialises");
            if new == json {
                return json;
            }
            json = new;
        }
    }
}

/// Rounds up to a multiple of 8.
pub fn align8(n: u64) -> u64 {
    (n + 7) & !7
}

#[inline]
pub fn seg_start(s: u16) -> u16 {
    s >> START_SHIFT
}

#[inline]
pub fn seg_code(s: u16) -> u8 {
    (s & CODE_MASK) as u8
}

#[inline]
pub fn pack_seg(start: u16, code: u8) -> u16 {
    (start << START_SHIFT) | code as u16
}
