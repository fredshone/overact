"""Reader and writer for the `.overact` format. See docs/FORMAT.md."""

from __future__ import annotations

import json
import shutil
import struct
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, BinaryIO

import numpy as np

MAGIC = b"OVERACT1"
VERSION = 1
MAX_AXIS_LEN = 2048
MAX_ACTIVITIES = 32
START_SHIFT = 5
CODE_MASK = 0x1F

DTYPES: dict[str, np.dtype] = {
    "u8": np.dtype("<u1"),
    "u16": np.dtype("<u2"),
    "u32": np.dtype("<u4"),
    "i8": np.dtype("<i1"),
    "i16": np.dtype("<i2"),
    "i32": np.dtype("<i4"),
    "f32": np.dtype("<f4"),
    "f64": np.dtype("<f8"),
    "utf8": np.dtype("u1"),
}

# Null sentinel per dtype. None means NaN.
NULLS: dict[str, int | None] = {
    "u8": 255,
    "u16": 65535,
    "i8": -128,
    "i16": -32768,
    "i32": -2147483648,
    "f32": None,
    "f64": None,
}


def pack_segments(starts_rel: np.ndarray, codes: np.ndarray) -> np.ndarray:
    """Pack start minutes (relative to the axis start) and activity codes into u16."""
    return ((starts_rel.astype(np.uint16) << START_SHIFT) | codes.astype(np.uint16)).astype("<u2")


def unpack_segments(segments: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Inverse of `pack_segments`: returns (start_rel, code)."""
    return (segments >> START_SHIFT).astype(np.int32), (segments & CODE_MASK).astype(np.uint8)


def _align8(n: int) -> int:
    return (n + 7) & ~7


@dataclass
class Blob:
    """A blob to write: either an in-memory array or a raw file to copy."""

    dtype: str
    nbytes: int
    array: np.ndarray | None = None
    path: Path | None = None

    @classmethod
    def from_array(cls, dtype: str, arr: np.ndarray) -> Blob:
        arr = np.ascontiguousarray(arr, dtype=DTYPES[dtype])
        return cls(dtype=dtype, nbytes=arr.nbytes, array=arr)

    def write_to(self, f: BinaryIO) -> None:
        if self.array is not None:
            f.write(memoryview(self.array).cast("B"))
        else:
            assert self.path is not None
            with open(self.path, "rb") as src:
                shutil.copyfileobj(src, f, 16 << 20)


@dataclass
class ColumnSpec:
    """A column ready to write: header metadata (without byte positions) and data."""

    meta: dict[str, Any]
    blob: Blob


def write_file(
    path: str | Path,
    *,
    n_persons: int,
    n_segments: int,
    time_axis: tuple[int, int],
    activities: list[str],
    offsets: Blob,
    segments: Blob,
    person_ids: tuple[Blob, Blob] | None,
    columns: list[ColumnSpec],
    meta: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Write an `.overact` file. Returns the header that was written."""
    a0, a1 = time_axis
    if not 1 <= a1 - a0 <= MAX_AXIS_LEN:
        raise ValueError(f"time axis length must be 1..{MAX_AXIS_LEN}, got {a1 - a0}")
    if len(activities) > MAX_ACTIVITIES:
        raise ValueError(f"at most {MAX_ACTIVITIES} activities, got {len(activities)}")
    if offsets.nbytes != 4 * (n_persons + 1) or segments.nbytes != 2 * n_segments:
        raise ValueError("offsets/segments size does not match n_persons/n_segments")
    names = [c.meta["name"] for c in columns]
    if len(set(names)) != len(names):
        raise ValueError("duplicate column names")

    blobs: list[tuple[dict[str, Any], Blob]] = []
    blob_table: dict[str, dict[str, Any]] = {}
    for name, blob in [("offsets", offsets), ("segments", segments)] + (
        [("person_id_offsets", person_ids[0]), ("person_id_data", person_ids[1])]
        if person_ids
        else []
    ):
        entry = {"dtype": blob.dtype, "byteOffset": 0, "byteLength": blob.nbytes}
        blob_table[name] = entry
        blobs.append((entry, blob))
    col_entries = []
    for c in columns:
        entry = {
            **c.meta,
            "dtype": c.blob.dtype,
            "byteOffset": 0,
            "byteLength": c.blob.nbytes,
        }
        col_entries.append(entry)
        blobs.append((entry, c.blob))

    header = {
        "format": "overact",
        "version": VERSION,
        "n_persons": int(n_persons),
        "n_segments": int(n_segments),
        "time_axis": {"start": int(a0), "end": int(a1), "unit": "minute"},
        "activities": [{"code": i, "label": lab} for i, lab in enumerate(activities)],
        "blobs": blob_table,
        "columns": col_entries,
        "meta": meta or {},
    }

    # Byte offsets depend on the header length, which depends on the offsets'
    # digit count. Iterate until stable (converges in two or three rounds).
    header_bytes = b""
    for _ in range(10):
        pos = _align8(12 + len(header_bytes))
        for entry, blob in blobs:
            entry["byteOffset"] = pos
            pos = _align8(pos + blob.nbytes)
        new = json.dumps(header, separators=(",", ":"), allow_nan=False).encode()
        if new == header_bytes:
            break
        header_bytes = new
    else:  # pragma: no cover
        raise RuntimeError("header layout did not converge")

    with open(path, "wb") as f:
        f.write(MAGIC)
        f.write(struct.pack("<I", len(header_bytes)))
        f.write(header_bytes)
        for entry, blob in blobs:
            f.write(b"\0" * (entry["byteOffset"] - f.tell()))
            blob.write_to(f)
        f.write(b"\0" * (_align8(f.tell()) - f.tell()))
    return header


def read_header(path: str | Path) -> dict[str, Any]:
    with open(path, "rb") as f:
        prelude = f.read(12)
        if len(prelude) < 12 or prelude[:8] != MAGIC:
            raise ValueError(f"{path}: not an .overact file (bad magic)")
        (hlen,) = struct.unpack("<I", prelude[8:12])
        return json.loads(f.read(hlen))


@dataclass
class OveractFile:
    """A fully or partially loaded `.overact` file (memory-mapped)."""

    path: Path
    header: dict[str, Any]
    _mm: np.memmap = field(repr=False)

    @classmethod
    def open(cls, path: str | Path) -> OveractFile:
        path = Path(path)
        header = read_header(path)
        mm = np.memmap(path, dtype=np.uint8, mode="r")
        return cls(path, header, mm)

    def _blob(self, entry: dict[str, Any]) -> np.ndarray:
        off, n = entry["byteOffset"], entry["byteLength"]
        return self._mm[off : off + n].view(DTYPES[entry["dtype"]])

    @property
    def n_persons(self) -> int:
        return self.header["n_persons"]

    @property
    def axis(self) -> tuple[int, int]:
        t = self.header["time_axis"]
        return t["start"], t["end"]

    @property
    def activity_labels(self) -> list[str]:
        return [a["label"] for a in self.header["activities"]]

    @property
    def offsets(self) -> np.ndarray:
        return self._blob(self.header["blobs"]["offsets"])

    @property
    def segments(self) -> np.ndarray:
        return self._blob(self.header["blobs"]["segments"])

    def person_ids(self) -> list[str] | None:
        b = self.header["blobs"]
        if "person_id_offsets" not in b:
            return None
        offs = self._blob(b["person_id_offsets"])
        data = self._blob(b["person_id_data"]).tobytes()
        return [data[offs[i] : offs[i + 1]].decode() for i in range(len(offs) - 1)]

    def column_meta(self, name: str) -> dict[str, Any]:
        for c in self.header["columns"]:
            if c["name"] == name:
                return c
        raise KeyError(name)

    def raw_column(self, name: str) -> np.ndarray:
        return self._blob(self.column_meta(name))

    def column(self, name: str) -> list[Any]:
        """Decoded column values as Python objects (None for missing)."""
        meta = self.column_meta(name)
        raw = self.raw_column(name)
        null = meta["nullValue"]
        if meta["kind"] == "numeric":
            if null is None:
                return [None if np.isnan(v) else float(v) for v in raw]
            return [None if v == null else int(v) for v in raw]
        if meta["kind"] == "boolean":
            return [None if v == null else bool(v) for v in raw]
        cats = meta["categories"]
        return [None if v == null else cats[v] for v in raw]

    def schedules(self) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
        """All segments as flat arrays: (person index, start, end, code), absolute minutes."""
        offs = self.offsets.astype(np.int64)
        seg = np.asarray(self.segments)
        a0, a1 = self.axis
        start_rel, code = unpack_segments(seg)
        person = np.repeat(np.arange(self.n_persons, dtype=np.int64), np.diff(offs))
        end_rel = np.empty_like(start_rel)
        end_rel[:-1] = start_rel[1:]
        if len(end_rel):
            last = offs[1:] - 1
            end_rel[last[np.diff(offs) > 0]] = a1 - a0
        return person, start_rel + a0, end_rel + a0, code
