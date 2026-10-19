//! An in-memory dataset: schedules plus lazily loaded columns.

use crate::column::{as_bytes, as_bytes_mut, Column, ColumnData};
use crate::format::{
    align8, seg_code, seg_start, BlobRef, DType, Error, Header, Result, MAGIC, PRELUDE_LEN,
};
use std::io::Write;

/// Which part of the file a byte range belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Offsets,
    Segments,
    PersonIdOffsets,
    PersonIdData,
    Column(usize),
}

/// A dataset. Schedules are always loaded; columns are loaded on first use.
///
/// Buffers are allocated up front (`Dataset::new`) and then filled in place
/// through `part_bytes_mut`, so a host (e.g. a browser) can copy file slices
/// straight into them without an intermediate buffer.
pub struct Dataset {
    pub header: Header,
    pub offsets: Vec<u32>,
    pub segments: Vec<u16>,
    person_id_offsets: Option<Vec<u32>>,
    person_id_data: Option<Vec<u8>>,
    columns: Vec<Option<Column>>,
    /// Whether each column has been filled and finished. A column's buffer
    /// exists from its first `part_bytes_mut`, but holds zeros until then.
    ready: Vec<bool>,
    checked: bool,
}

impl Dataset {
    /// Allocates (zeroed) schedule buffers for `header`. Fill them with
    /// `part_bytes_mut(Part::Offsets / Part::Segments)`, then call `finish_schedules`.
    pub fn new(header: Header) -> Dataset {
        let n = header.n_persons as usize;
        let columns: Vec<Option<Column>> = header.columns.iter().map(|_| None).collect();
        Dataset {
            offsets: vec![0; n + 1],
            segments: vec![0; header.n_segments as usize],
            person_id_offsets: None,
            person_id_data: None,
            ready: vec![false; columns.len()],
            columns,
            header,
            checked: false,
        }
    }

    /// Loads a whole file held in memory (tests and native tools).
    pub fn from_bytes(bytes: &[u8]) -> Result<Dataset> {
        let header = Header::from_file_start(bytes)?;
        let mut ds = Dataset::new(header);
        let mut parts = vec![Part::Offsets, Part::Segments];
        if ds.header.blobs.person_id_offsets.is_some() {
            parts.extend([Part::PersonIdOffsets, Part::PersonIdData]);
        }
        for p in parts {
            let r = ds.blob(p)?;
            let src = slice(bytes, &r)?;
            ds.part_bytes_mut(p)?.copy_from_slice(src);
        }
        ds.finish_schedules()?;
        for i in 0..ds.header.columns.len() {
            let r = ds.blob(Part::Column(i))?;
            let src = slice(bytes, &r)?;
            ds.part_bytes_mut(Part::Column(i))?.copy_from_slice(src);
            ds.finish_column(i)?;
        }
        Ok(ds)
    }

    /// Builds a dataset from arrays already in memory (e.g. parsed from
    /// tables). Blob lengths in `header` are set from the data and offsets are
    /// laid out as `write` would; `header.columns[i]` describes `columns[i]`
    /// (its dtype is taken from the data). Schedules are validated.
    pub fn from_parts(
        mut header: Header,
        offsets: Vec<u32>,
        segments: Vec<u16>,
        person_ids: Option<(Vec<u32>, Vec<u8>)>,
        columns: Vec<ColumnData>,
    ) -> Result<Dataset> {
        let blob = |dtype: DType, n: usize| BlobRef {
            dtype,
            byte_offset: 0,
            byte_length: (n * dtype.size()) as u64,
        };
        let n = offsets.len().saturating_sub(1);
        header.n_persons = n as u32;
        header.n_segments = segments.len() as u32;
        header.blobs.offsets = blob(DType::U32, offsets.len());
        header.blobs.segments = blob(DType::U16, segments.len());
        header.blobs.person_id_offsets =
            person_ids.as_ref().map(|(o, _)| blob(DType::U32, o.len()));
        header.blobs.person_id_data = person_ids.as_ref().map(|(_, d)| blob(DType::Utf8, d.len()));
        if columns.len() != header.columns.len() {
            return Err(Error::Invalid(
                "column count does not match the header".into(),
            ));
        }
        for (m, c) in header.columns.iter_mut().zip(&columns) {
            if c.len() != n {
                return Err(Error::Invalid(format!(
                    "column {:?} has the wrong length",
                    m.name
                )));
            }
            m.dtype = c.dtype();
            m.byte_length = (n * m.dtype.size()) as u64;
        }
        if let Some((o, d)) = &person_ids {
            if o.len() != n + 1 || o[0] != 0 || o.last().map(|&x| x as usize) != Some(d.len()) {
                return Err(Error::Invalid("bad person id offsets".into()));
            }
        }
        header.layout();
        header.check()?;
        let cols = header
            .columns
            .iter()
            .zip(columns)
            .map(|(m, c)| Column::new(m.clone(), c).map(Some))
            .collect::<Result<Vec<_>>>()?;
        let (person_id_offsets, person_id_data) = person_ids.unzip();
        let mut ds = Dataset {
            ready: vec![true; cols.len()],
            columns: cols,
            offsets,
            segments,
            person_id_offsets,
            person_id_data,
            header,
            checked: false,
        };
        ds.finish_schedules()?;
        Ok(ds)
    }

    /// Writes the dataset as an `.overact` file. Every column must be loaded.
    pub fn write(&self, w: &mut impl Write) -> Result<()> {
        let parts = self.parts()?;
        let mut header = self.header.clone();
        let json = header.layout();
        let io = |e: std::io::Error| Error::Invalid(e.to_string());
        w.write_all(MAGIC).map_err(io)?;
        w.write_all(&(json.len() as u32).to_le_bytes())
            .map_err(io)?;
        w.write_all(&json).map_err(io)?;
        let mut pos = (PRELUDE_LEN + json.len()) as u64;
        let zeros = [0u8; 8];
        let blobs = self.blob_refs(&header);
        for (b, bytes) in blobs.iter().zip(parts) {
            w.write_all(&zeros[..(b.byte_offset - pos) as usize])
                .map_err(io)?;
            w.write_all(bytes).map_err(io)?;
            pos = b.byte_offset + bytes.len() as u64;
        }
        w.write_all(&zeros[..(align8(pos) - pos) as usize])
            .map_err(io)?;
        Ok(())
    }

    /// Blob refs in file order (as `Header::layout` places them).
    fn blob_refs(&self, h: &Header) -> Vec<BlobRef> {
        let b = &h.blobs;
        let mut v = vec![b.offsets.clone(), b.segments.clone()];
        v.extend(b.person_id_offsets.clone());
        v.extend(b.person_id_data.clone());
        v.extend(h.columns.iter().map(|c| c.blob()));
        v
    }

    /// The bytes of every blob, in file order. Little-endian hosts only.
    fn parts(&self) -> Result<Vec<&[u8]>> {
        if cfg!(target_endian = "big") {
            return Err(Error::Invalid("writing needs a little-endian host".into()));
        }
        let mut v = vec![as_bytes(&self.offsets), as_bytes(&self.segments)];
        if self.header.blobs.person_id_offsets.is_some() {
            let none = || Error::Invalid("person ids not loaded".into());
            v.push(as_bytes(self.person_id_offsets.as_ref().ok_or_else(none)?));
            v.push(self.person_id_data.as_ref().ok_or_else(none)?);
        }
        for i in 0..self.header.columns.len() {
            let c = self
                .column(i)
                .ok_or_else(|| Error::Invalid(format!("column {i} not loaded")))?;
            v.push(c.data.bytes());
        }
        Ok(v)
    }

    pub fn n_persons(&self) -> usize {
        self.header.n_persons as usize
    }

    pub fn axis_len(&self) -> u32 {
        self.header.axis_len()
    }

    pub fn n_activities(&self) -> usize {
        self.header.activities.len()
    }

    /// The byte range of a part within the file.
    pub fn blob(&self, part: Part) -> Result<BlobRef> {
        let b = &self.header.blobs;
        let none = || Error::Invalid("no person ids".into());
        Ok(match part {
            Part::Offsets => b.offsets.clone(),
            Part::Segments => b.segments.clone(),
            Part::PersonIdOffsets => b.person_id_offsets.clone().ok_or_else(none)?,
            Part::PersonIdData => b.person_id_data.clone().ok_or_else(none)?,
            Part::Column(i) => self
                .header
                .columns
                .get(i)
                .ok_or_else(|| Error::UnknownColumn(format!("#{i}")))?
                .blob(),
        })
    }

    /// The bytes of a part if it is loaded (native byte order).
    pub fn part_bytes(&self, part: Part) -> Option<&[u8]> {
        match part {
            Part::Offsets => Some(as_bytes(&self.offsets)),
            Part::Segments => Some(as_bytes(&self.segments)),
            Part::PersonIdOffsets => self.person_id_offsets.as_deref().map(as_bytes),
            Part::PersonIdData => self.person_id_data.as_deref(),
            Part::Column(i) => self.column(i).map(|c| c.data.bytes()),
        }
    }

    /// Writable bytes for a part, allocating column / person-id storage on demand.
    pub fn part_bytes_mut(&mut self, part: Part) -> Result<&mut [u8]> {
        Ok(match part {
            // Finished parts are read by engine jobs without further checks,
            // so they are immutable.
            Part::Offsets | Part::Segments if self.checked => {
                return Err(Error::Invalid("schedules are already loaded".into()));
            }
            Part::Offsets => as_bytes_mut(&mut self.offsets),
            Part::Segments => as_bytes_mut(&mut self.segments),
            Part::PersonIdOffsets => {
                let n = self.n_persons() + 1;
                as_bytes_mut(self.person_id_offsets.get_or_insert_with(|| vec![0; n]))
            }
            Part::PersonIdData => {
                let len = self
                    .header
                    .blobs
                    .person_id_data
                    .as_ref()
                    .ok_or_else(|| Error::Invalid("no person ids".into()))?
                    .byte_length as usize;
                self.person_id_data
                    .get_or_insert_with(|| vec![0; len])
                    .as_mut_slice()
            }
            Part::Column(i) => {
                let meta = self
                    .header
                    .columns
                    .get(i)
                    .ok_or_else(|| Error::UnknownColumn(format!("#{i}")))?
                    .clone();
                if self.ready[i] {
                    return Err(Error::Invalid(format!(
                        "column {:?} is already loaded",
                        meta.name
                    )));
                }
                let n = self.n_persons();
                if self.columns[i].is_none() {
                    self.columns[i] = Some(Column::alloc(meta, n)?);
                }
                self.columns[i].as_mut().unwrap().data.bytes_mut()
            }
        })
    }

    /// Validates the schedule arrays after they were filled. Must succeed
    /// before any engine function touches the schedules.
    pub fn finish_schedules(&mut self) -> Result<()> {
        #[cfg(target_endian = "big")]
        {
            self.offsets.iter_mut().for_each(|x| *x = u32::from_le(*x));
            self.segments.iter_mut().for_each(|x| *x = u16::from_le(*x));
        }
        let bad = |s: &str| Err(Error::Invalid(s.into()));
        let o = &self.offsets;
        if o[0] != 0 || *o.last().unwrap() as usize != self.segments.len() {
            return bad("offsets must run from 0 to n_segments");
        }
        if o.windows(2).any(|w| w[1] <= w[0]) {
            return bad("every person needs at least one segment");
        }
        let n_act = self.n_activities() as u8;
        let len = self.axis_len() as u16;
        for p in 0..self.n_persons() {
            let segs = &self.segments[o[p] as usize..o[p + 1] as usize];
            if seg_start(segs[0]) != 0 {
                return bad("a person's first segment must start at 0");
            }
            let mut prev = None;
            for &s in segs {
                let (t, c) = (seg_start(s), seg_code(s));
                if c >= n_act || t >= len || prev.is_some_and(|pt| t <= pt) {
                    return bad("segments must have increasing starts and valid codes");
                }
                prev = Some(t);
            }
        }
        self.checked = true;
        Ok(())
    }

    pub fn schedules_ready(&self) -> bool {
        self.checked
    }

    /// Marks a column filled through `part_bytes_mut` as ready.
    pub fn finish_column(&mut self, i: usize) -> Result<()> {
        let c = self
            .columns
            .get_mut(i)
            .ok_or_else(|| Error::UnknownColumn(format!("#{i}")))?
            .as_mut()
            .ok_or_else(|| Error::Invalid("column not filled".into()))?;
        c.data.fix_endian();
        self.ready[i] = true;
        Ok(())
    }

    pub fn column(&self, i: usize) -> Option<&Column> {
        self.columns
            .get(i)
            .filter(|_| self.ready[i])
            .and_then(|c| c.as_ref())
    }

    /// Whether column `i` is filled and finished (usable by the engine).
    pub fn column_loaded(&self, i: usize) -> bool {
        self.column(i).is_some()
    }

    /// Drops a column's data (e.g. under memory pressure). Not while an
    /// engine job that uses the column is queued: jobs expect it loaded.
    pub fn unload_column(&mut self, i: usize) {
        if i < self.columns.len() {
            self.columns[i] = None;
            self.ready[i] = false;
        }
    }

    /// The packed segments of person `p`.
    #[inline]
    pub fn person(&self, p: usize) -> &[u16] {
        &self.segments[self.offsets[p] as usize..self.offsets[p + 1] as usize]
    }

    /// Iterates `(start, end, code)` for person `p`, in minutes from the axis start.
    pub fn person_segments(&self, p: usize) -> impl Iterator<Item = (u16, u16, u8)> + '_ {
        let segs = self.person(p);
        let len = self.axis_len() as u16;
        segs.iter().enumerate().map(move |(k, &s)| {
            let end = segs.get(k + 1).map_or(len, |&n| seg_start(n));
            (seg_start(s), end, seg_code(s))
        })
    }

    pub fn person_id(&self, p: usize) -> Option<&str> {
        let o = self.person_id_offsets.as_ref()?;
        let d = self.person_id_data.as_ref()?;
        let (a, b) = (*o.get(p)? as usize, *o.get(p + 1)? as usize);
        std::str::from_utf8(d.get(a..b)?).ok()
    }

    /// Total minutes spent in each activity, over all persons (by code).
    pub fn activity_minutes(&self) -> Vec<u64> {
        let mut out = vec![0u64; self.n_activities()];
        let len = self.axis_len() as u16;
        for p in 0..self.n_persons() {
            let segs = self.person(p);
            for (k, &s) in segs.iter().enumerate() {
                let end = segs.get(k + 1).map_or(len, |&n| seg_start(n));
                out[seg_code(s) as usize] += (end - seg_start(s)) as u64;
            }
        }
        out
    }

    pub fn has_person_ids(&self) -> bool {
        self.person_id_offsets.is_some() && self.person_id_data.is_some()
    }
}

fn slice<'a>(bytes: &'a [u8], r: &BlobRef) -> Result<&'a [u8]> {
    let a = r.byte_offset as usize;
    bytes
        .get(a..a + r.byte_length as usize)
        .ok_or(Error::Truncated)
}
