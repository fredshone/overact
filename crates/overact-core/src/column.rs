//! Typed attribute columns, plus derived (virtual) columns.

use crate::format::{ColumnKind, ColumnMeta, DType, Error, Result};

/// The values of one column, in person order.
#[derive(Debug, Clone)]
pub enum ColumnData {
    U8(Vec<u8>),
    U16(Vec<u16>),
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

macro_rules! each {
    ($self:expr, $v:ident => $e:expr) => {
        match $self {
            ColumnData::U8($v) => $e,
            ColumnData::U16($v) => $e,
            ColumnData::I8($v) => $e,
            ColumnData::I16($v) => $e,
            ColumnData::I32($v) => $e,
            ColumnData::F32($v) => $e,
            ColumnData::F64($v) => $e,
        }
    };
}

/// Views a slice of plain numbers as bytes. All column element types are
/// plain-old-data with no padding, so this is sound.
pub(crate) fn as_bytes_mut<T: Copy>(v: &mut [T]) -> &mut [u8] {
    let len = std::mem::size_of_val(v);
    // SAFETY: T is a primitive number type (u8..f64); any bit pattern is valid.
    unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, len) }
}

/// Views a slice of plain numbers as bytes (native byte order).
pub(crate) fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: as in `as_bytes_mut`.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

impl ColumnData {
    pub fn zeroed(dtype: DType, n: usize) -> Result<ColumnData> {
        Ok(match dtype {
            DType::U8 => ColumnData::U8(vec![0; n]),
            DType::U16 => ColumnData::U16(vec![0; n]),
            DType::I8 => ColumnData::I8(vec![0; n]),
            DType::I16 => ColumnData::I16(vec![0; n]),
            DType::I32 => ColumnData::I32(vec![0; n]),
            DType::F32 => ColumnData::F32(vec![0.0; n]),
            DType::F64 => ColumnData::F64(vec![0.0; n]),
            DType::U32 | DType::Utf8 => {
                return Err(Error::Invalid(format!("{dtype:?} is not a column dtype")))
            }
        })
    }

    pub fn len(&self) -> usize {
        each!(self, v => v.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn dtype(&self) -> DType {
        match self {
            ColumnData::U8(_) => DType::U8,
            ColumnData::U16(_) => DType::U16,
            ColumnData::I8(_) => DType::I8,
            ColumnData::I16(_) => DType::I16,
            ColumnData::I32(_) => DType::I32,
            ColumnData::F32(_) => DType::F32,
            ColumnData::F64(_) => DType::F64,
        }
    }

    /// The raw storage in native byte order.
    pub fn bytes(&self) -> &[u8] {
        each!(self, v => as_bytes(v.as_slice()))
    }

    /// The raw storage as little-endian bytes, for filling straight from a file.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        each!(self, v => as_bytes_mut(v.as_mut_slice()))
    }

    /// Fixes byte order after a raw fill on big-endian hosts (no-op on LE).
    pub fn fix_endian(&mut self) {
        #[cfg(target_endian = "big")]
        match self {
            ColumnData::U8(_) | ColumnData::I8(_) => {}
            ColumnData::U16(v) => v.iter_mut().for_each(|x| *x = u16::from_le(*x)),
            ColumnData::I16(v) => v.iter_mut().for_each(|x| *x = i16::from_le(*x)),
            ColumnData::I32(v) => v.iter_mut().for_each(|x| *x = i32::from_le(*x)),
            ColumnData::F32(v) => v
                .iter_mut()
                .for_each(|x| *x = f32::from_bits(u32::from_le(x.to_bits()))),
            ColumnData::F64(v) => v
                .iter_mut()
                .for_each(|x| *x = f64::from_bits(u64::from_le(x.to_bits()))),
        }
    }
}

/// How a column's values map to sort keys and filters.
#[derive(Debug, Clone)]
pub struct Column {
    pub meta: ColumnMeta,
    pub data: ColumnData,
    /// Null sentinel for integer types (NaN is used for floats).
    null: i64,
}

/// Sort key for missing values: sorts last in both directions.
pub const NULL_KEY: u64 = u64::MAX;

impl Column {
    pub fn new(meta: ColumnMeta, data: ColumnData) -> Result<Column> {
        let null = match (&data, meta.null_value) {
            (ColumnData::F32(_) | ColumnData::F64(_), _) => 0,
            (_, Some(v)) => v as i64,
            (_, None) => {
                return Err(Error::Invalid(format!(
                    "column {:?} needs a nullValue",
                    meta.name
                )))
            }
        };
        Ok(Column { meta, data, null })
    }

    /// Allocates an empty column of `n` rows, to be filled through `data.bytes_mut()`.
    pub fn alloc(meta: ColumnMeta, n: usize) -> Result<Column> {
        let data = ColumnData::zeroed(meta.dtype, n)?;
        Column::new(meta, data)
    }

    pub fn from_le_bytes(meta: ColumnMeta, bytes: &[u8]) -> Result<Column> {
        let n = bytes.len() / meta.dtype.size();
        let mut c = Column::alloc(meta, n)?;
        c.data
            .bytes_mut()
            .copy_from_slice(&bytes[..n * c.meta.dtype.size()]);
        c.data.fix_endian();
        Ok(c)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn kind(&self) -> ColumnKind {
        self.meta.kind
    }

    /// The value as f64 (category index for categoricals), or None if missing.
    #[inline]
    pub fn get(&self, i: usize) -> Option<f64> {
        let n = self.null;
        match &self.data {
            ColumnData::U8(v) => (v[i] as i64 != n).then_some(v[i] as f64),
            ColumnData::U16(v) => (v[i] as i64 != n).then_some(v[i] as f64),
            ColumnData::I8(v) => (v[i] as i64 != n).then_some(v[i] as f64),
            ColumnData::I16(v) => (v[i] as i64 != n).then_some(v[i] as f64),
            ColumnData::I32(v) => (v[i] as i64 != n).then_some(v[i] as f64),
            ColumnData::F32(v) => (!v[i].is_nan()).then_some(v[i] as f64),
            ColumnData::F64(v) => (!v[i].is_nan()).then_some(v[i]),
        }
    }

    #[inline]
    pub fn is_null(&self, i: usize) -> bool {
        self.get(i).is_none()
    }

    /// Bits needed by `sort_key` for this column (16, 32 or 64).
    pub fn key_bits(&self) -> u32 {
        match &self.data {
            ColumnData::U8(_) | ColumnData::I8(_) | ColumnData::U16(_) | ColumnData::I16(_) => 16,
            ColumnData::I32(_) | ColumnData::F32(_) => 32,
            ColumnData::F64(_) => 64,
        }
    }

    /// Order-preserving unsigned key. Missing values map to `NULL_KEY`.
    /// Non-null keys are below `2^key_bits - 1`, so `NULL_KEY` truncated to
    /// `key_bits` is still the largest key.
    #[inline]
    pub fn sort_key(&self, i: usize) -> u64 {
        let n = self.null;
        match &self.data {
            ColumnData::U8(v) => {
                if v[i] as i64 == n {
                    NULL_KEY
                } else {
                    v[i] as u64
                }
            }
            ColumnData::U16(v) => {
                if v[i] as i64 == n {
                    NULL_KEY
                } else {
                    v[i] as u64
                }
            }
            // Signed: the sentinel is the type minimum, so shifting by
            // (-min - 1) maps real values to 0..=2^bits-2.
            ColumnData::I8(v) => {
                if v[i] as i64 == n {
                    NULL_KEY
                } else {
                    (v[i] as i64 + 127) as u64
                }
            }
            ColumnData::I16(v) => {
                if v[i] as i64 == n {
                    NULL_KEY
                } else {
                    (v[i] as i64 + 32767) as u64
                }
            }
            ColumnData::I32(v) => {
                if v[i] as i64 == n {
                    NULL_KEY
                } else {
                    (v[i] as i64 + 2147483647) as u64
                }
            }
            ColumnData::F32(v) => f32_key(v[i]).map_or(NULL_KEY, |k| k as u64),
            ColumnData::F64(v) => f64_key(v[i]).unwrap_or(NULL_KEY),
        }
    }
}

/// Monotone u32 key for a float; None for NaN. -0.0 and 0.0 map to the same key.
#[inline]
pub fn f32_key(x: f32) -> Option<u32> {
    if x.is_nan() {
        return None;
    }
    let x = if x == 0.0 { 0.0 } else { x };
    let b = x.to_bits();
    // Positive floats get the sign bit set; negative floats are inverted.
    // +inf maps to 0xFF80_0000, so u32::MAX stays free for nulls.
    Some(if b & 0x8000_0000 != 0 {
        !b
    } else {
        b | 0x8000_0000
    })
}

#[inline]
pub fn f64_key(x: f64) -> Option<u64> {
    if x.is_nan() {
        return None;
    }
    let x = if x == 0.0 { 0.0 } else { x };
    let b = x.to_bits();
    Some(if b & (1 << 63) != 0 {
        !b
    } else {
        b | (1 << 63)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_keys_are_monotone() {
        let xs = [
            f32::NEG_INFINITY,
            -3.5,
            -1e-30,
            -0.0,
            0.0,
            1e-30,
            2.0,
            f32::INFINITY,
        ];
        let ks: Vec<u32> = xs.iter().map(|&x| f32_key(x).unwrap()).collect();
        for w in ks.windows(2) {
            assert!(w[0] <= w[1]);
        }
        assert!(ks.iter().all(|&k| k != u32::MAX));
        assert_eq!(f32_key(f32::NAN), None);
    }
}
