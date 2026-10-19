"""overact-validate: check a `.overact` file against docs/FORMAT.md."""

from __future__ import annotations

import argparse
import os
import struct
import sys
from pathlib import Path

import numpy as np

from .format import (
    DTYPES,
    MAGIC,
    MAX_ACTIVITIES,
    MAX_AXIS_LEN,
    NULLS,
    OveractFile,
    unpack_segments,
)


def validate(path: str | Path) -> list[str]:
    """Return a list of problems (empty if the file is valid)."""
    path = Path(path)
    errors: list[str] = []
    size = os.path.getsize(path)
    with open(path, "rb") as f:
        pre = f.read(12)
    if pre[:8] != MAGIC:
        return ["bad magic"]
    (hlen,) = struct.unpack("<I", pre[8:12])
    if 12 + hlen > size:
        return ["header runs past end of file"]
    try:
        of = OveractFile.open(path)
    except Exception as e:  # noqa: BLE001
        return [f"cannot parse header: {e}"]
    h = of.header
    if h.get("format") != "overact" or h.get("version") != 1:
        errors.append("format/version must be 'overact'/1")
    n, ns = h["n_persons"], h["n_segments"]
    a0, a1 = of.axis
    if not 1 <= a1 - a0 <= MAX_AXIS_LEN:
        errors.append(f"time axis length {a1 - a0} not in 1..{MAX_AXIS_LEN}")
    acts = h["activities"]
    if len(acts) > MAX_ACTIVITIES:
        errors.append(f"{len(acts)} activities > {MAX_ACTIVITIES}")
    if [a["code"] for a in acts] != list(range(len(acts))):
        errors.append("activity codes must be 0..len-1 in order")
    if len({a["label"] for a in acts}) != len(acts):
        errors.append("activity labels must be unique")

    def check_blob(name: str, e: dict, expect_len: int | None) -> bool:
        ok = True
        if e["dtype"] not in DTYPES:
            errors.append(f"{name}: unknown dtype {e['dtype']}")
            return False
        if e["byteOffset"] % 8:
            errors.append(f"{name}: byteOffset not 8-aligned")
            ok = False
        if e["byteOffset"] < 12 + hlen or e["byteOffset"] + e["byteLength"] > size:
            errors.append(f"{name}: byte range outside the data section")
            ok = False
        if expect_len is not None and e["byteLength"] != expect_len * DTYPES[e["dtype"]].itemsize:
            errors.append(f"{name}: byteLength does not match length {expect_len}")
            ok = False
        return ok

    b = h["blobs"]
    if not (
        check_blob("offsets", b["offsets"], n + 1) and check_blob("segments", b["segments"], ns)
    ):
        return errors
    offs = of.offsets.astype(np.int64)
    if offs[0] != 0 or offs[-1] != ns:
        errors.append("offsets must start at 0 and end at n_segments")
    counts = np.diff(offs)
    if (counts < 1).any():
        errors.append(f"{int((counts < 1).sum())} persons have no segments")
    if not errors:
        start, code = unpack_segments(np.asarray(of.segments))
        firsts = offs[:-1]
        if (start[firsts] != 0).any():
            errors.append(f"{int((start[firsts] != 0).sum())} persons do not start at 0")
        same = np.ones(ns, bool)
        same[firsts] = False
        same[0] = False
        prev_start = np.roll(start, 1)
        if (same & (start <= prev_start)).any():
            errors.append(f"{int((same & (start <= prev_start)).sum())} non-increasing starts")
        if (start >= a1 - a0).any():
            errors.append("segment starts at or after the axis end")
        if (code >= len(acts)).any():
            errors.append("segment code outside the activity list")

    if ("person_id_offsets" in b) != ("person_id_data" in b):
        errors.append("person_id_offsets and person_id_data must appear together")
    elif (
        "person_id_offsets" in b
        and check_blob("person_id_offsets", b["person_id_offsets"], n + 1)
        and check_blob("person_id_data", b["person_id_data"], None)
    ):
        po = of._blob(b["person_id_offsets"]).astype(np.int64)
        if po[0] != 0 or po[-1] != b["person_id_data"]["byteLength"] or (np.diff(po) < 0).any():
            errors.append("person id offsets are inconsistent")

    names = set()
    for c in h["columns"]:
        name = c["name"]
        if name in names:
            errors.append(f"duplicate column {name!r}")
        names.add(name)
        if not check_blob(f"column {name!r}", c, n):
            continue
        dt, kind, null = c["dtype"], c["kind"], c["nullValue"]
        allowed = {
            "numeric": {"f32", "f64", "i8", "i16", "i32"},
            "categorical": {"u8", "u16"},
            "boolean": {"u8"},
        }
        if dt not in allowed.get(kind, set()):
            errors.append(f"column {name!r}: dtype {dt} not allowed for kind {kind}")
            continue
        if null != NULLS[dt]:
            errors.append(f"column {name!r}: nullValue must be {NULLS[dt]}")
        raw = np.asarray(of.raw_column(name))
        isnull = np.isnan(raw) if null is None else raw == null
        valid = raw[~isnull]
        if int(isnull.sum()) != c.get("nullCount", int(isnull.sum())):
            errors.append(
                f"column {name!r}: nullCount is {c['nullCount']}, found {int(isnull.sum())}"
            )
        if kind == "categorical" and len(valid) and valid.max() >= len(c["categories"]):
            errors.append(f"column {name!r}: category index out of range")
        if kind == "boolean" and len(valid) and valid.max() > 1:
            errors.append(f"column {name!r}: boolean values must be 0 or 1")
        if kind == "numeric" and len(valid):
            lo, hi = valid.min(), valid.max()
            if c["min"] is not None and (
                lo != np.asarray(c["min"], raw.dtype) or hi != np.asarray(c["max"], raw.dtype)
            ):
                errors.append(
                    f"column {name!r}: min/max {c['min']}/{c['max']} but data has {lo}/{hi}"
                )
    return errors


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="overact-validate", description=__doc__)
    p.add_argument("files", nargs="+", type=Path)
    args = p.parse_args(argv)
    bad = 0
    for path in args.files:
        errs = validate(path)
        if errs:
            bad += 1
            print(f"{path}: INVALID")
            for e in errs:
                print(f"  - {e}")
        else:
            h = OveractFile.open(path).header
            print(
                f"{path}: ok ({h['n_persons']:,} persons, {h['n_segments']:,} segments, "
                f"{len(h['columns'])} columns)"
            )
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
