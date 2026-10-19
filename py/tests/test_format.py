import json
import struct

import numpy as np
import polars as pl
import pytest

from overact.format import OveractFile, read_header
from overact.pack import pack
from overact.validate import validate


def _write(tmp_path, acts, attrs=None, **kw):
    a = tmp_path / "acts.csv"
    pl.DataFrame(acts, schema=["pid", "act", "start", "end"], orient="row").write_csv(a)
    b = None
    if attrs is not None:
        b = tmp_path / "attrs.csv"
        pl.DataFrame(attrs).write_csv(b)
    out = tmp_path / "out.overact"
    res = pack(a, b, out, verbose=False, **kw)
    return out, res


def test_worked_example(tmp_path):
    """The example in docs/FORMAT.md section 5."""
    acts = [
        ("a", "home", 0, 480),
        ("a", "work", 480, 1020),
        ("a", "home", 1020, 1440),
        ("b", "home", 0, 1440),
        ("c", "home", 0, 600),
        ("c", "shop", 600, 660),
        ("c", "home", 660, 1440),
    ]
    attrs = {"pid": ["a", "b", "c"], "age": [34, 61, None]}
    out, res = _write(tmp_path, acts, attrs)
    f = OveractFile.open(out)
    assert f.activity_labels == ["home", "work", "shop"]
    assert f.offsets.tolist() == [0, 3, 4, 7]
    assert f.segments.tolist() == [0, 15361, 32640, 0, 0, 19202, 21120]
    age = f.column_meta("age")
    assert (age["dtype"], age["nullValue"], age["nullCount"], age["min"], age["max"]) == (
        "i8",
        -128,
        1,
        34,
        61,
    )
    assert f.raw_column("age").tolist() == [34, 61, -128]
    assert f.person_ids() == ["a", "b", "c"]
    assert not res.fixes
    assert validate(out) == []

    raw = out.read_bytes()
    assert raw[:8] == b"OVERACT1"
    (hlen,) = struct.unpack("<I", raw[8:12])
    header = json.loads(raw[12 : 12 + hlen])
    for blob in [*header["blobs"].values(), *header["columns"]]:
        assert blob["byteOffset"] % 8 == 0


def test_fixes_are_reported(tmp_path):
    acts = [
        ("p", "home", 0, 300),
        ("p", "work", 400, 900),  # gap 300-400
        ("p", "shop", 850, 950),  # overlap, trimmed to 900
        ("p", "shop", 860, 870),  # fully covered, dropped
        ("p", "home", 950, 1000),
        ("p", "home", 1000, 1300),  # kept separate; trailing gap 1300-1440
        ("q", "work", 500, 400),  # empty
        ("q", "work", -60, 60),  # clipped
        ("q", "home", 60, 1500),  # clipped
        ("r", "home", 1500, 1600),  # outside; r then has nothing
    ]
    attrs = {"pid": ["p", "q", "r", "s"], "x": [1.5, None, 2.0, 3.0]}
    out, res = _write(tmp_path, acts, attrs)
    assert dict(res.fixes) == {
        "gap_filled": 2,
        "overlap_trimmed": 1,
        "overlap_dropped": 1,
        "dropped_empty": 1,
        "clipped": 2,
        "dropped_outside": 1,
        "person_filled": 2,
    }
    f = OveractFile.open(out)
    assert "unknown" in f.activity_labels
    person, start, end, code = f.schedules()
    labels = f.activity_labels
    got = [(f.person_ids()[p], labels[c], s, e) for p, s, e, c in zip(person, start, end, code)]
    assert got == [
        ("p", "home", 0, 300),
        ("p", "unknown", 300, 400),
        ("p", "work", 400, 900),
        ("p", "shop", 900, 950),
        ("p", "home", 950, 1000),
        ("p", "home", 1000, 1300),
        ("p", "unknown", 1300, 1440),
        ("q", "work", 0, 60),
        ("q", "home", 60, 1440),
        ("r", "unknown", 0, 1440),
        ("s", "unknown", 0, 1440),
    ]
    assert validate(out) == []


def test_unsorted_input_and_extra_persons(tmp_path):
    acts = [
        ("z", "work", 600, 1440),
        ("y", "home", 0, 1440),
        ("z", "home", 0, 600),
        ("extra", "home", 0, 1440),
    ]
    out, res = _write(tmp_path, acts, {"pid": ["z", "y"], "flag": [True, False]})
    f = OveractFile.open(out)
    assert f.person_ids() == ["z", "y", "extra"]
    assert f.column("flag") == [True, False, None]
    assert res.extra_persons == 1
    assert res.warnings and "no attributes" in res.warnings[0]
    assert not res.fixes


def test_duplicate_attribute_ids(tmp_path):
    with pytest.raises(ValueError, match="duplicate"):
        _write(tmp_path, [("a", "home", 0, 1440)], {"pid": ["a", "a"]})


def test_custom_axis_and_duration(tmp_path):
    a = tmp_path / "a.csv"
    pl.DataFrame({"id": [1, 1], "label": ["sleep", "awake"], "s": [4, 10], "d": [6, 18]}).write_csv(
        a
    )
    out = tmp_path / "o.overact"
    pack(
        a,
        None,
        out,
        id_col="id",
        act_col="label",
        start_col="s",
        end_col=None,
        duration_col="d",
        time_unit="hour",
        axis=(240, 1680),
        verbose=False,
    )
    f = OveractFile.open(out)
    assert f.axis == (240, 1680)
    _, start, end, _code = f.schedules()
    assert start.tolist() == [240, 600] and end.tolist() == [600, 1680]
    assert validate(out) == []


def test_axis_limits(tmp_path):
    with pytest.raises(ValueError, match="axis"):
        _write(tmp_path, [("a", "home", 0, 1440)], axis=(0, 3000))


def test_validate_detects_corruption(tmp_path):
    out, _ = _write(tmp_path, [("a", "home", 0, 600), ("a", "work", 600, 1440)])
    h = read_header(out)
    raw = bytearray(out.read_bytes())
    seg = h["blobs"]["segments"]["byteOffset"]
    raw[seg + 2 : seg + 4] = struct.pack("<H", (0 << 5) | 1)  # second start = 0
    out.write_bytes(bytes(raw))
    assert any("non-increasing" in e for e in validate(out))


def _reference_grid(acts, ids, axis, labels):
    """Naive per-minute painter: earlier start wins, gaps are unknown."""
    a0, a1 = axis
    grid = np.full((len(ids), a1 - a0), -1, np.int64)
    pos = {p: i for i, p in enumerate(ids)}
    for p, act, s, e, seq in sorted(acts, key=lambda r: (pos[r[0]], r[2], r[4])):
        if s >= e:
            continue
        row = grid[pos[p]]
        lo, hi = max(s, a0) - a0, min(e, a1) - a0
        if lo < hi:
            seg = row[lo:hi]
            seg[seg < 0] = labels.index(act)
    unk = labels.index("unknown") if "unknown" in labels else -1
    grid[grid < 0] = unk
    return grid


def _decoded_grid(f):
    a0, a1 = f.axis
    person, start, end, code = f.schedules()
    grid = np.full((f.n_persons, a1 - a0), -2, np.int64)
    for p, s, e, c in zip(person, start, end, code):
        grid[p, s - a0 : e - a0] = c
    return grid


@pytest.mark.parametrize("seed", range(20))
def test_random_messy_round_trip(tmp_path, seed):
    """Random messy input: packed result equals the naive minute painter."""
    rng = np.random.default_rng(seed)
    n = 30
    acts = []
    for p in range(n):
        for k in range(rng.integers(0, 8)):
            s = int(rng.integers(-100, 1500))
            e = s + int(rng.integers(-20, 400))
            acts.append((f"p{p}", str(rng.choice(["a", "b", "c"])), s, e, k))
    rng.shuffle(acts)
    src = tmp_path / "acts.parquet"
    pl.DataFrame(acts, schema=["pid", "act", "start", "end", "seq"], orient="row").write_parquet(
        src
    )
    ids = [f"p{p}" for p in range(n)]
    attrs = tmp_path / "attrs.parquet"
    pl.DataFrame({"pid": ids}).write_parquet(attrs)
    out = tmp_path / "out.overact"
    pack(src, attrs, out, seq_col="seq", verbose=False, bucket_persons=7)
    assert validate(out) == []
    f = OveractFile.open(out)
    assert f.person_ids() == ids
    labels = f.activity_labels
    np.testing.assert_array_equal(_decoded_grid(f), _reference_grid(acts, ids, (0, 1440), labels))


def test_report_lists_every_fix(tmp_path):
    # 30 gaps: more than the examples kept in the header.
    acts = [(f"p{i}", "home", 0, 600) for i in range(30)]
    report = tmp_path / "fixes.csv"
    out, res = _write(tmp_path, acts, report_path=report)
    assert res.fixes["gap_filled"] == 30
    rows = pl.read_csv(report)
    assert rows.columns == ["fix", "person_id", "start", "end", "activity"]
    assert rows.filter(pl.col("fix") == "gap_filled").height == 30
    examples = read_header(out)["meta"]["fix_examples"]
    assert 0 < len(examples) < 30


def test_f64_columns(tmp_path):
    acts = [("a", "home", 0, 1440), ("b", "home", 0, 1440)]
    attrs = {"pid": ["a", "b"], "x": [0.1, 1e300], "y": [0.1, 2.5]}
    out, _ = _write(tmp_path, acts, attrs, f64_cols=["x"])
    f = OveractFile.open(out)
    assert f.column_meta("x")["dtype"] == "f64"
    assert f.column_meta("y")["dtype"] == "f32"
    assert f.column("x") == [0.1, 1e300]
