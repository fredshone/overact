"""Write the small synthetic source tables in py/tests/fixtures.

2,000 persons from overact-synth, reshaped into a wider survey-like layout so
the tests cover string ids, an id-like column that gets skipped (hid),
all-null and constant columns, nullable integers and floats, and a per-row
string column on the schedules. Run make_rust_fixtures.py afterwards.
"""

from pathlib import Path

import numpy as np
import polars as pl

from overact.synth import ACTS, gen_attributes, gen_schedules, to_tables

FIX = Path(__file__).resolve().parents[1] / "tests" / "fixtures"
N = 2000
SEED = 7


def main() -> None:
    rng = np.random.default_rng(SEED)
    attrs = gen_attributes(rng, N, 0)
    rows, starts, codes = gen_schedules(rng, attrs)
    a, s = to_tables(attrs, rows, starts, codes)
    a, s = pl.from_arrow(a), pl.from_arrow(s)

    pid = pl.Series("pid", [f"p{i:05d}" for i in range(N)])
    age = a["age"].cast(pl.Int64)
    relationship = np.where(
        age < 18, "child", rng.choice(["self", "partner", "other"], N, p=[0.55, 0.4, 0.05])
    )
    null_str = pl.Series([None] * N, dtype=pl.String)
    speed = np.round(rng.lognormal(2.5, 0.6, N), 4)
    attributes = pl.DataFrame(
        {
            "hid": pid,
            "pid": pid,
            "age": age,
            "hh_size": a["hh_size"].cast(pl.Int64),
            "hh_income": a["income"].fill_nan(None).cast(pl.Int64),
            "sex": a["sex"],
            "dwelling": null_str,
            "ownership": null_str,
            "vehicles": a["vehicles"].cast(pl.Int64),
            "education": a["education"],
            "occupation": null_str,
            "group": rng.choice(["a", "b", "c"], N, p=[0.7, 0.2, 0.1]),
            "has_licence": np.where(a["has_licence"].to_numpy(), "yes", "no"),
            "relationship": relationship,
            "employment": a["employment"],
            "source": ["synth"] * N,
            "year": pl.Series([2020] * N, dtype=pl.Int64),
            "month": pl.Series(rng.integers(1, 4, N), dtype=pl.Int64),
            "day": a["day"],
            "hh_zone": a["zone"],
            "weight": a["weight"].cast(pl.Float64),
            "avg_speed": pl.Series(np.where(rng.random(N) < 0.4, np.nan, speed)).fill_nan(None),
        }
    )

    # Activity zone: mostly the home zone, sometimes elsewhere when away.
    home_zone = a["zone"].to_numpy()[s["pid"].to_numpy()]
    away = (s["act"] != "home").to_numpy() & (rng.random(s.height) < 0.3)
    zone = np.where(away, rng.choice(["urban", "suburban", "rural"], s.height), home_zone)
    schedules = pl.DataFrame(
        {
            "pid": pid.gather(s["pid"]),
            "seq": s["seq"].cast(pl.Int64),
            "act": s["act"].cast(pl.Enum(ACTS)).cast(pl.String),
            "zone": zone,
            "start": s["start"].cast(pl.Int64),
            "end": s["end"].cast(pl.Int64),
        }
    )

    FIX.mkdir(parents=True, exist_ok=True)
    for name, df in (("schedules", schedules), ("attributes", attributes)):
        df.write_parquet(FIX / f"synth_small_{name}.parquet")
        df.write_csv(FIX / f"synth_small_{name}.csv")
    print(f"wrote {N} persons, {schedules.height} segments to {FIX}")


if __name__ == "__main__":
    main()
