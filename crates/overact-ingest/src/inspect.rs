//! A quick look at a table for the column-mapping form: names, types and the
//! first few rows.

use crate::attrs::{infer, Kind};
use crate::table::{ColData, ColType, Format, Table};
use crate::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ColType,
}

#[derive(Debug, Serialize)]
pub struct Inspection {
    pub format: Format,
    pub columns: Vec<ColumnInfo>,
    /// First rows, as text (`None` for missing).
    pub preview: Vec<Vec<Option<String>>>,
    /// Row count when the format records it.
    pub n_rows: Option<u64>,
    pub bytes: u64,
}

/// Rows used to guess CSV column types.
const SAMPLE_ROWS: usize = 1000;

pub fn inspect(table: &Table, preview_rows: usize) -> Result<Inspection> {
    let names = table.names().to_vec();
    let mut types = table.types();
    let head = table.head(preview_rows.max(SAMPLE_ROWS))?;
    if table.format() == Format::Csv {
        for (c, col) in head.cols.iter().enumerate() {
            if let ColData::Text(t) = col {
                types[c] = match infer((0..t.len()).map(|i| t.get(i))) {
                    Kind::Int => ColType::Int,
                    Kind::Float => ColType::Float,
                    Kind::Bool => ColType::Bool,
                    Kind::Text => ColType::Text,
                };
            }
        }
    }
    let mut buf = Vec::new();
    let preview = (0..head.len.min(preview_rows))
        .map(|i| {
            head.cols
                .iter()
                .map(|c| {
                    c.key(i, &mut buf)
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                })
                .collect()
        })
        .collect();
    Ok(Inspection {
        format: table.format(),
        columns: names
            .into_iter()
            .zip(types)
            .map(|(name, ty)| ColumnInfo { name, ty })
            .collect(),
        preview,
        n_rows: table.n_rows(),
        bytes: table.byte_len(),
    })
}
