from pathlib import Path

import numpy as np
import polars as pl

from overact.format import OveractFile
from overact.pack import pack
from overact.synth import generate
from overact.validate import validate

FIX = Path(__file__).parent / "fixtures"


def _pack(tmp_path, ext):
    out = tmp_path / f"small_{ext}.overact"
    res = pack(
        FIX / f"synth_small_schedules.{ext}",
        FIX / f"synth_small_attributes.{ext}",
        out,
        seq_col="seq",
        verbose=False,
    )
    return out, res


def test_small_csv_and_parquet_agree(tmp_path):
    out_csv, res = _pack(tmp_path, "csv")
    out_pq, _ = _pack(tmp_path, "parquet")
    assert validate(out_csv) == [] and validate(out_pq) == []
    a, b = OveractFile.open(out_csv), OveractFile.open(out_pq)
    assert a.offsets.tolist() == b.offsets.tolist()
    assert a.segments.tolist() == b.segments.tolist()
    names = [c["name"] for c in a.header["columns"]]
    assert names == [c["name"] for c in b.header["columns"]]
    for name in names:
        assert a.column(name) == b.column(name), name
    # The sample has zero-length rows; nothing else needs fixing.
    assert set(res.fixes) <= {"dropped_empty"}
    assert any("hid" in w for w in res.warnings)


def test_small_matches_source(tmp_path):
    out, _ = _pack(tmp_path, "parquet")
    f = OveractFile.open(out)
    src = pl.read_parquet(FIX / "synth_small_schedules.parquet").filter(
        pl.col("end") > pl.col("start")
    )
    attrs = pl.read_parquet(FIX / "synth_small_attributes.parquet")
    ids = f.person_ids()
    assert ids == attrs["pid"].to_list()
    labels = f.activity_labels
    pos = {p: i for i, p in enumerate(ids)}
    want = np.full((len(ids), 1440), -1)
    for p, act, s, e in src.select("pid", "act", "start", "end").iter_rows():
        want[pos[p], s:e] = labels.index(act)
    got = np.full((len(ids), 1440), -2)
    person, start, end, code = f.schedules()
    # Back-to-back repeats stay separate: one segment per kept row.
    assert len(start) == src.height
    for p, s, e, c in zip(person, start, end, code):
        got[p, s:e] = c
    np.testing.assert_array_equal(got, want)
    # Attribute values survive exactly (f32 columns within f32 precision).
    assert f.column("age") == attrs["age"].to_list()
    assert f.column("employment") == attrs["employment"].to_list()
    assert f.column("hh_income") == attrs["hh_income"].to_list()
    w = np.array(f.column("weight"), dtype=np.float32)
    np.testing.assert_array_equal(w, attrs["weight"].cast(pl.Float32).to_numpy())


def test_synth_is_clean(tmp_path):
    a, s = generate(5000, tmp_path, seed=1, chunk=2000, verbose=False)
    out = tmp_path / "synth.overact"
    res = pack(s, a, out, seq_col="seq", verbose=False)
    assert validate(out) == []
    assert not res.fixes, res.fixes
    f = OveractFile.open(out)
    assert f.n_persons == 5000
    assert 5 < f.header["n_segments"] / 5000 < 10
    # Correlations the viewer demo relies on.
    emp = np.array(f.column("employment"), dtype=object)
    age = np.array(f.column("age"))
    assert (age[emp == "retired"] >= 55).all()
    assert (age[emp == "child"] < 5).all()
