"""Write the cross-language test fixture used by crates/overact-core/tests.

Packs the small synthetic sample (from make_test_tables.py) and writes, next
to it, a JSON file with every schedule and decoded column value as the Python
reader sees them. The Rust tests check that the Rust reader produces exactly
the same values.
"""

import json
import math
from pathlib import Path

import polars as pl

from overact.format import OveractFile
from overact.pack import pack

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "py" / "tests" / "fixtures"
OUT = ROOT / "crates" / "overact-core" / "tests" / "data"


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / "synth_small.overact"
    pack(
        FIX / "synth_small_schedules.parquet",
        FIX / "synth_small_attributes.parquet",
        path,
        seq_col="seq",
        verbose=False,
    )
    f = OveractFile.open(path)
    person, start, end, code = f.schedules()
    schedules = [[] for _ in range(f.n_persons)]
    for p, s, e, c in zip(person.tolist(), start.tolist(), end.tolist(), code.tolist()):
        schedules[p].append([s, e, c])
    columns = {}
    for meta in f.header["columns"]:
        vals = f.column(meta["name"])
        if meta["kind"] == "categorical":
            cats = meta["categories"]
            vals = [None if v is None else cats.index(v) for v in vals]
        elif meta["kind"] == "boolean":
            vals = [None if v is None else int(v) for v in vals]
        columns[meta["name"]] = [
            None if v is None or (isinstance(v, float) and math.isnan(v)) else v for v in vals
        ]
    # M4 acceptance: persons working at 09:00 and at home at 20:00, computed
    # straight from the source table (not from the packed file).
    src = pl.read_parquet(FIX / "synth_small_schedules.parquet").filter(
        pl.col("end") > pl.col("start")
    )

    def doing(act: str, minute: int) -> set[str]:
        hit = src.filter(
            (pl.col("act") == act) & (pl.col("start") <= minute) & (pl.col("end") > minute)
        )
        return set(hit["pid"].to_list())

    work9_home20 = sorted(doing("work", 9 * 60) & doing("home", 20 * 60))
    expected = {
        "ids": f.person_ids(),
        "schedules": schedules,
        "columns": columns,
        "work_0900_home_2000": work9_home20,
    }
    (OUT / "synth_small.expected.json").write_text(json.dumps(expected))
    print(f"wrote {path} and expected values")


if __name__ == "__main__":
    main()
