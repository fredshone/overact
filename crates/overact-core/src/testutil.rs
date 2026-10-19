//! Builds small in-memory datasets for tests.

use crate::column::ColumnData;
use crate::dataset::Part;
use crate::format::{pack_seg, Header};
use crate::Dataset;
use serde_json::json;

/// `schedules[p]` is a list of (start, code) with increasing starts, first
/// start 0 (codes may repeat).
pub fn dataset(axis_len: u32, n_act: usize, schedules: &[Vec<(u16, u8)>]) -> Dataset {
    dataset_with_columns(axis_len, n_act, schedules, vec![])
}

/// A test column: (name, kind, categories, data). Nulls use the format's sentinels.
pub type TestColumn = (&'static str, &'static str, Option<Vec<String>>, ColumnData);

pub fn dataset_with_columns(
    axis_len: u32,
    n_act: usize,
    schedules: &[Vec<(u16, u8)>],
    columns: Vec<TestColumn>,
) -> Dataset {
    let n = schedules.len();
    let n_seg: usize = schedules.iter().map(|s| s.len()).sum();
    let cols: Vec<_> = columns
        .iter()
        .map(|(name, kind, cats, data)| {
            let (dtype, null_value, size) = match data {
                ColumnData::U8(_) => ("u8", json!(255), 1),
                ColumnData::U16(_) => ("u16", json!(65535), 2),
                ColumnData::I8(_) => ("i8", json!(-128), 1),
                ColumnData::I16(_) => ("i16", json!(-32768), 2),
                ColumnData::I32(_) => ("i32", json!(-2147483648i64), 4),
                ColumnData::F32(_) => ("f32", json!(null), 4),
                ColumnData::F64(_) => ("f64", json!(null), 8),
            };
            json!({"name": name, "kind": kind, "dtype": dtype, "byteOffset": 0,
                   "byteLength": size * n, "nullValue": null_value, "min": null, "max": null,
                   "categories": cats})
        })
        .collect();
    let h = json!({
        "format": "overact", "version": 1,
        "n_persons": n, "n_segments": n_seg,
        "time_axis": {"start": 0, "end": axis_len},
        "activities": (0..n_act).map(|i| json!({"code": i, "label": format!("a{i}")})).collect::<Vec<_>>(),
        "blobs": {
            "offsets": {"dtype": "u32", "byteOffset": 0, "byteLength": 4 * (n + 1)},
            "segments": {"dtype": "u16", "byteOffset": 0, "byteLength": 2 * n_seg},
        },
        "columns": cols,
    });
    let header = Header::parse(h.to_string().as_bytes()).unwrap();
    let mut ds = Dataset::new(header);
    let mut k = 0;
    for (p, s) in schedules.iter().enumerate() {
        ds.offsets[p] = k as u32;
        for &(t, c) in s {
            ds.segments[k] = pack_seg(t, c);
            k += 1;
        }
    }
    ds.offsets[n] = k as u32;
    ds.finish_schedules().unwrap();
    for (i, (_, _, _, mut data)) in columns.into_iter().enumerate() {
        ds.part_bytes_mut(Part::Column(i))
            .unwrap()
            .copy_from_slice(data.bytes_mut());
        ds.finish_column(i).unwrap();
    }
    ds
}

/// One-segment persons with the given person ids.
pub fn dataset_with_ids(axis_len: u32, ids: &[&str]) -> Dataset {
    let base = dataset(axis_len, 1, &vec![vec![(0, 0)]; ids.len()]);
    let mut offsets = vec![0u32];
    let mut data = Vec::new();
    for id in ids {
        data.extend_from_slice(id.as_bytes());
        offsets.push(data.len() as u32);
    }
    Dataset::from_parts(
        base.header.clone(),
        base.offsets.clone(),
        base.segments.clone(),
        Some((offsets, data)),
        vec![],
    )
    .unwrap()
}
