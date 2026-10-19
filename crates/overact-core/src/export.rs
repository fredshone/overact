//! Person ids of a row list, for exporting a view back to analysis tools.
//! Persons without a stored id are written as their index.

use crate::dataset::Dataset;

/// Appends person `p`'s id (or index) to `out`.
fn push_id(ds: &Dataset, p: u32, out: &mut Vec<u8>) {
    match ds.person_id(p as usize) {
        Some(id) => out.extend_from_slice(id.as_bytes()),
        None => out.extend_from_slice(p.to_string().as_bytes()),
    }
}

/// `person_id,row` CSV of `rows` in order (ids quoted when needed).
pub fn ids_csv(ds: &Dataset, rows: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.len() * 16 + 16);
    out.extend_from_slice(b"person_id,row\n");
    let mut id = Vec::new();
    for (i, &p) in rows.iter().enumerate() {
        id.clear();
        push_id(ds, p, &mut id);
        if id.iter().any(|&b| matches!(b, b'"' | b',' | b'\n' | b'\r')) {
            out.push(b'"');
            for &b in &id {
                if b == b'"' {
                    out.push(b'"');
                }
                out.push(b);
            }
            out.push(b'"');
        } else {
            out.extend_from_slice(&id);
        }
        out.push(b',');
        out.extend_from_slice(i.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

/// The ids of `rows` in order as UTF-8 strings: `(offsets[n + 1], data)`.
pub fn ids_utf8(ds: &Dataset, rows: &[u32]) -> (Vec<u32>, Vec<u8>) {
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut data = Vec::with_capacity(rows.len() * 8);
    offsets.push(0);
    for &p in rows {
        push_id(ds, p, &mut data);
        offsets.push(data.len() as u32);
    }
    (offsets, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::dataset;

    #[test]
    fn csv_and_utf8_ids() {
        let ds = dataset(10, 1, &[vec![(0, 0)], vec![(0, 0)], vec![(0, 0)]]);
        // The test dataset has no stored ids: indices are used.
        assert_eq!(ids_csv(&ds, &[2, 0]), b"person_id,row\n2,0\n0,1\n");
        let (o, d) = ids_utf8(&ds, &[2, 0, 1]);
        assert_eq!(o, vec![0, 1, 2, 3]);
        assert_eq!(d, b"201");
    }

    #[test]
    fn activity_minutes_sum_durations() {
        let ds = dataset(10, 2, &[vec![(0, 0), (4, 1)], vec![(0, 1)]]);
        assert_eq!(ds.activity_minutes(), vec![4, 16]);
    }

    #[test]
    fn csv_quotes_ids() {
        let ds = crate::testutil::dataset_with_ids(10, &["a,b", "q\"x", "plain"]);
        assert_eq!(
            ids_csv(&ds, &[0, 1, 2]),
            b"person_id,row\n\"a,b\",0\n\"q\"\"x\",1\nplain,2\n"
        );
    }
}
