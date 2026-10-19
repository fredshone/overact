# The `.overact` file format (version 1)

This document is the authoritative specification. The Python writer
(`py/src/overact/format.py`), the Python reader and the Rust reader
(`crates/overact-core/src/format.rs`) all follow it.

An `.overact` file holds one day of activities for each of `N` persons plus any
number of per-person attribute columns. It is designed to be read lazily: a
reader fetches the small JSON header first, then only the byte ranges it needs
(`File.slice()` in the browser, HTTP Range requests, or `mmap` natively).

All multi-byte numbers are **little-endian**.

## 1. Layout

| Offset | Size | Content |
|---|---|---|
| 0 | 8 | ASCII magic `OVERACT1` |
| 8 | 4 | `u32` header length `H` in bytes |
| 12 | `H` | UTF-8 JSON header |
| 12 + `H` | 0–7 | zero padding up to a multiple of 8 |
| … | … | blobs, each starting at an 8-byte-aligned absolute offset |

Every `byteOffset` in the header is **absolute** (from the start of the file)
and a multiple of 8. Blobs may appear in any order and may be separated by
padding. A reader must not assume anything about bytes that no blob covers.

## 2. Header

```jsonc
{
  "format": "overact",
  "version": 1,
  "n_persons": 3,
  "n_segments": 7,
  "time_axis": { "start": 0, "end": 1440, "unit": "minute" },
  "activities": [ { "code": 0, "label": "home" }, { "code": 1, "label": "work" } ],
  "blobs": {
    "offsets":           { "dtype": "u32",  "byteOffset": 512, "byteLength": 16 },
    "segments":          { "dtype": "u16",  "byteOffset": 528, "byteLength": 12 },
    "person_id_offsets": { "dtype": "u32",  "byteOffset": 544, "byteLength": 16 },
    "person_id_data":    { "dtype": "utf8", "byteOffset": 560, "byteLength": 3 }
  },
  "columns": [ /* see section 4 */ ],
  "meta": { /* free-form: source files, packer version, fix report, … */ }
}
```

- `time_axis.end - time_axis.start` must be between 1 and 2048 inclusive.
  `start` may be negative or larger than 1440 (e.g. 240 to 1680 for a 04:00 to
  04:00 day).
- `activities` lists every code that may appear, at most 32 entries, with codes
  `0..len-1` in order. Labels are unique. The packer orders activities by total
  time spent, descending, and adds `unknown` when it had to fill gaps.
- `person_id_offsets` / `person_id_data` are optional (both or neither).
- Unknown header keys must be ignored by readers.

## 3. Schedules

Schedules are stored in CSR (compressed sparse row) form.

- `offsets`: `u32[N + 1]`. Person `i` owns segments
  `offsets[i] .. offsets[i+1]`. `offsets[0] = 0`,
  `offsets[N] = n_segments`, non-decreasing. Every person has at least one
  segment.
- `segments`: `u16[n_segments]`. Each value packs one segment:

  ```
  bits 15..5  start  (11 bits) minutes since time_axis.start, 0..2047
  bits  4..0  code   (5 bits)  activity code, 0..31
  ```

  `value = (start << 5) | code`.

Within a person, segment starts are strictly increasing and the first start is
`0`. A segment ends where the next one starts; the last segment ends at
`time_axis.end`. So a person's segments tile the whole axis with no gaps or
overlaps. Consecutive segments of one person may have the same code: the
packer keeps back-to-back rows of one activity as separate segments (episodes).

## 4. Columns

Each entry of `columns` describes one per-person attribute stored as an array
of `N` values in person order:

```jsonc
{
  "name": "age",
  "kind": "numeric",          // "numeric" | "categorical" | "boolean"
  "dtype": "i8",              // see table below
  "byteOffset": 4096,
  "byteLength": 3,            // N * size_of(dtype)
  "nullValue": -128,          // sentinel for missing values; null means NaN (floats only)
  "nullCount": 0,
  "min": 23, "max": 61,       // over non-null values; null when every value is null
  "categories": ["female", "male"]   // categorical only
}
```

| kind | dtypes | null | meaning |
|---|---|---|---|
| `numeric` | `f32`, `f64` | NaN (`nullValue` is `null`) | the value |
| `numeric` | `i8`, `i16`, `i32` | the type's minimum (`-128`, `-32768`, `-2147483648`) | the value |
| `categorical` | `u8`, `u16` | the type's maximum (`255`, `65535`) | index into `categories` |
| `boolean` | `u8` | `255` | `0` false, `1` true |

`categories` is in the default display/sort order (the packer sorts them
naturally, so `"2" < "10"`). Integer columns never contain their sentinel as a
real value; the packer widens the type instead. A column whose values are all
missing still has data bytes (all sentinel). Column names are unique.
`nullValue` must be exactly the sentinel in the table above; the Rust reader
rejects files where it is not.

Readers should load a column's bytes only when it is first used.

## 5. Worked example

Three persons on the default 0 to 1440 axis:

| person | schedule |
|---|---|
| `a` | home 0–480, work 480–1020, home 1020–1440 |
| `b` | home 0–1440 |
| `c` | home 0–600, shop 600–660, home 660–1440 |

Activities are coded by total time: `home`=0, `work`=1, `shop`=2.

```
offsets  = [0, 3, 4, 7]                               u32[4]
segments = [ (0<<5)|0, (480<<5)|1, (1020<<5)|0,        a
             (0<<5)|0,                                 b
             (0<<5)|0, (600<<5)|2, (660<<5)|0 ]        c
         = [0, 15361, 32640, 0, 0, 19202, 21120]       u16[7]
```

With an `age` column of `[34, 61, null]` stored as `i8`:
`[34, 61, -128]`, `nullValue: -128`, `nullCount: 1`, `min: 34`, `max: 61`.

The test fixture `py/tests/test_format.py::test_worked_example` builds exactly
this file and checks these bytes.
