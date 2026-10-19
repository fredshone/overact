"""overact-synth: generate a synthetic population with structured, correlated schedules.

Attributes are correlated (age drives employment, education and income;
zone drives vehicles and licences), and schedules follow from them: employed
people work on most weekdays, with start times that shift with age; children
go to school; retired people make shorter trips. Every trip between places is
a `travel` segment, which gives roughly 7 to 9 segments per person.

Generation is vectorised and chunked, so 10M persons need about 1 GB of RAM.
"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.csv as pacsv
import pyarrow.parquet as pq

ACTS = [
    "home",
    "work",
    "education",
    "shop",
    "leisure",
    "visit",
    "escort",
    "medical",
    "other",
    "travel",
]
A = {name: i for i, name in enumerate(ACTS)}
DAYS = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"]
ZONES = ["urban", "suburban", "rural"]
EDU = ["none", "school", "high-school", "degree"]
EMP = ["ft-employed", "pt-employed", "unemployed", "student", "retired", "other", "child"]
E = {name: i for i, name in enumerate(EMP)}
DAY_END = 1440


def _choice(rng: np.random.Generator, probs: np.ndarray) -> np.ndarray:
    """Row-wise categorical draw: probs has shape (n, k)."""
    cum = np.cumsum(probs, axis=1)
    cum /= cum[:, -1:]
    return (rng.random((len(probs), 1)) > cum).sum(axis=1)


def gen_attributes(rng: np.random.Generator, n: int, first_id: int) -> dict[str, np.ndarray]:
    # Age: flat to 60 then declining.
    age_w = np.where(np.arange(95) < 60, 1.0, np.exp(-(np.arange(95) - 60) / 12))
    age = rng.choice(95, size=n, p=age_w / age_w.sum()).astype(np.int16)
    sex = rng.integers(0, 2, n)
    zone = rng.choice(3, size=n, p=[0.5, 0.3, 0.2])
    day = rng.choice(7, size=n)
    weekend = day >= 5

    # Education (adults only).
    p_degree = np.clip(0.45 - np.abs(age - 35) / 120, 0.1, 0.45)
    p_edu = np.stack([0.05 + 0 * age, 0.35 - p_degree / 2, 0.6 - p_degree / 2, p_degree], 1)
    edu = _choice(rng, p_edu)
    edu_null = age < 18

    # Employment by age band.
    p_emp = np.zeros((n, len(EMP)))
    child = age < 5
    school = (age >= 5) & (age < 16)
    young = (age >= 16) & (age < 23)
    adult = (age >= 23) & (age < 65)
    old = age >= 65
    p_emp[child, E["child"]] = 1
    p_emp[school, E["student"]] = 1
    p_emp[young] = [0.2, 0.2, 0.08, 0.5, 0, 0.02, 0]
    deg = edu == 3
    p_emp[adult] = [0.55, 0.18, 0.07, 0.02, 0.0, 0.18, 0]
    p_emp[adult & deg] = [0.68, 0.14, 0.04, 0.03, 0.0, 0.11, 0]
    p_emp[adult & (age >= 58)] = [0.4, 0.15, 0.05, 0, 0.3, 0.1, 0]
    p_emp[old] = [0.04, 0.06, 0.0, 0, 0.85, 0.05, 0]
    emp = _choice(rng, p_emp)

    # Personal income, correlated with employment, education and age.
    base = np.select(
        [emp == E["ft-employed"], emp == E["pt-employed"], emp == E["retired"]],
        [42000.0, 21000.0, 24000.0],
        default=11000.0,
    )
    edu_mult = np.array([0.7, 0.85, 1.0, 1.45])[edu]
    age_mult = 0.6 + 0.4 * np.clip((age - 18) / 25, 0, 1) + 0.1 * np.clip((age - 45) / 15, 0, 1)
    income = base * edu_mult * age_mult * rng.lognormal(0, 0.35, n)
    income_null = (age < 16) | (rng.random(n) < 0.03)

    hh_size = np.clip(
        np.where(age < 18, rng.integers(3, 6, n), rng.poisson(1.4, n) + 1), 1, 8
    ).astype(np.int8)
    licence = (age >= 18) & (rng.random(n) < np.array([0.65, 0.85, 0.92])[zone])
    vehicles = (
        np.where(
            licence,
            rng.poisson(np.array([0.7, 1.3, 1.8])[zone] * np.clip(income / 40000, 0.5, 1.6)),
            rng.poisson(0.3, n),
        )
        .clip(0, 5)
        .astype(np.int8)
    )
    employed = (emp == E["ft-employed"]) | (emp == E["pt-employed"])
    can_wfh = rng.random(n) < np.where(deg, 0.6, 0.2)
    weight = rng.lognormal(0, 0.25, n).astype(np.float32)

    return {
        "pid": np.arange(first_id, first_id + n, dtype=np.int64),
        "age": age,
        "sex": np.array(["female", "male"])[sex],
        "employment": np.array(EMP)[emp],
        "education": np.where(edu_null, None, np.array(EDU, dtype=object)[edu]),
        "income": np.where(income_null, np.nan, np.round(income, -1)),
        "hh_size": hh_size,
        "vehicles": vehicles,
        "has_licence": licence,
        "can_wfh": np.where(employed, can_wfh, None),
        "zone": np.array(ZONES)[zone],
        "day": np.array(DAYS)[day],
        "weight": weight,
        # internal, not written:
        "_emp": emp,
        "_weekend": weekend,
        "_zone": zone,
    }


def gen_schedules(rng: np.random.Generator, attrs: dict[str, np.ndarray]):
    """Return flat arrays (person row, start, code) in person-then-time order."""
    n = len(attrs["pid"])
    age = attrs["age"].astype(np.float64)
    emp = attrs["_emp"]
    weekend = attrs["_weekend"]
    zone = attrs["_zone"]
    M = 5  # max out-of-home activities per day

    code = np.full((n, M), -1, np.int64)
    want = np.zeros((n, M))  # desired start
    dur = np.zeros((n, M))

    def put(mask, slot, c, start, d):
        code[mask, slot] = c if np.isscalar(c) else c[mask]
        want[mask, slot] = start[mask]
        dur[mask, slot] = d[mask]

    def normal(mu, sd):
        return rng.normal(mu, sd, n)

    ft = emp == E["ft-employed"]
    pt = emp == E["pt-employed"]
    works = (ft | pt) & np.where(weekend, rng.random(n) < 0.15, rng.random(n) < 0.9)
    works &= ~(attrs["can_wfh"].astype(bool) & (rng.random(n) < 0.25))
    # Older workers start earlier; part-timers start later.
    w_start = normal(8 * 60 + 20, 55) - (age - 40) * 1.2 + np.where(pt, 60, 0)
    w_start = np.where(rng.random(n) < 0.06, normal(14 * 60, 120), w_start)  # shift workers
    w_dur = np.where(ft, normal(8.6 * 60, 50), normal(4.6 * 60, 60)).clip(90, 12 * 60)
    lunch = works & ft & (rng.random(n) < 0.25)
    half = (w_dur / 2).round()
    put(works & ~lunch, 0, A["work"], w_start, w_dur)
    put(lunch, 0, A["work"], w_start, half)
    lunch_act = rng.choice([A["shop"], A["leisure"], A["other"]], n, p=[0.4, 0.4, 0.2])
    put(lunch, 1, lunch_act, w_start + half + 10, rng.uniform(20, 45, n))
    put(lunch, 2, A["work"], w_start + half + 70, w_dur - half - 60)
    after = np.where(lunch, 3, 1)

    pupil = (emp == E["student"]) & (age < 16) & ~weekend & (rng.random(n) < 0.96)
    uni = (emp == E["student"]) & (age >= 16) & ~weekend & (rng.random(n) < 0.8)
    put(pupil, 0, A["education"], normal(8 * 60 + 25, 12), normal(6.5 * 60, 30))
    put(uni, 0, A["education"], normal(10 * 60, 70), normal(5 * 60, 90).clip(60, 600))
    after = np.where(pupil | uni, 1, after)
    after = np.where(works | pupil | uni, after, 0)

    # Discretionary activities, more for people with free time.
    free = ~(works | pupil | uni)
    p_extra = np.where(free, np.where(emp == E["retired"], 0.85, 0.8), 0.5)
    p_extra = np.where(emp == E["child"], 0.4, p_extra)
    disc = rng.choice(
        [A["shop"], A["leisure"], A["visit"], A["escort"], A["medical"], A["other"]],
        (n, M),
        p=[0.3, 0.3, 0.14, 0.1, 0.04, 0.12],
    )
    disc = np.where((age < 30)[:, None] & (disc == A["medical"]), A["leisure"], disc)
    # Parents of school-age children escort in the morning.
    escort_am = (
        (age >= 28) & (age < 50) & (attrs["hh_size"] >= 3) & ~weekend & (rng.random(n) < 0.25)
    )
    for slot in range(M):
        go = (slot >= after) & (rng.random(n) < p_extra * (0.85 ** np.maximum(slot - after, 0)))
        if slot > 0:
            go &= (slot == after) | (code[:, slot - 1] >= 0)
        prev_end = np.where(slot > 0, want[:, max(slot - 1, 0)] + dur[:, max(slot - 1, 0)], 0)
        base = np.where(
            slot == after,
            np.where(
                free,
                normal(10 * 60 + 30, 120) - (age > 65) * 30,
                prev_end + rng.uniform(30, 120, n),
            ),
            prev_end + rng.exponential(90, n) + 20,
        )
        d = np.select(
            [disc[:, slot] == A["shop"], disc[:, slot] == A["escort"], disc[:, slot] == A["visit"]],
            [rng.uniform(15, 75, n), rng.uniform(5, 20, n), rng.uniform(60, 240, n)],
            default=rng.uniform(30, 180, n),
        )
        put(go & (code[:, slot] < 0), slot, disc[:, slot], base, d)
    # The escort takes slot 0, so work moves to slot 1.
    shift = escort_am & works & ~lunch
    put(shift, 0, A["escort"], w_start - 40, np.full(n, 10.0))
    put(shift, 1, A["work"], w_start, w_dur)

    # Build segments: home, travel, act, travel, (home, travel,) act, ..., travel, home.
    tt = np.array([18, 25, 32])[zone]  # typical one-way travel minutes
    starts = [np.zeros(n)]
    codes = [np.full(n, A["home"])]
    valid = [np.ones(n, bool)]
    t = np.zeros(n)  # end of the last placed activity
    at_home = np.ones(n, bool)
    for slot in range(M):
        c = code[:, slot]
        travel = np.maximum(3, rng.normal(tt, tt / 3)).round()
        s = np.maximum(want[:, slot].round(), t + travel)
        ok = (c >= 0) & (s + 10 < DAY_END - 2 * travel)
        # Long gap away from home: go home in between.
        go_home = ok & ~at_home & (s - t > 2 * travel + 60)
        home_travel = np.maximum(3, rng.normal(tt, tt / 3)).round()
        starts += [t, t + home_travel]
        codes += [np.full(n, A["travel"]), np.full(n, A["home"])]
        valid += [go_home, go_home]
        at_home = at_home | go_home
        e = np.minimum(s + np.maximum(dur[:, slot], 5).round(), DAY_END - travel - 2)
        starts += [s - travel, s]
        codes += [np.full(n, A["travel"]), c]
        valid += [ok, ok]
        t = np.where(ok, e, t)
        at_home &= ~ok
    back = ~at_home
    travel = np.maximum(3, rng.normal(tt, tt / 3)).round()
    starts += [t, np.minimum(t + travel, DAY_END - 1)]
    codes += [np.full(n, A["travel"]), np.full(n, A["home"])]
    valid += [back, back]

    S = np.stack(starts, 1).astype(np.int64)
    C = np.stack(codes, 1).astype(np.uint8)
    V = np.stack(valid, 1)
    # Times must strictly increase within a person; drop segments that would not.
    S = np.where(V, S, -1)
    run = np.maximum.accumulate(np.where(V, S, -1), axis=1)
    prev = np.concatenate([np.full((n, 1), -1), run[:, :-1]], axis=1)
    V &= S > prev
    V[:, 0] = True
    rows = np.repeat(np.arange(n), V.sum(1))
    return rows, S[V], C[V]


def to_tables(attrs: dict[str, np.ndarray], rows, starts, codes) -> tuple[pa.Table, pa.Table]:
    a = pa.table(
        {k: pa.array(v, from_pandas=True) for k, v in attrs.items() if not k.startswith("_")}
    )
    ends = np.empty_like(starts)
    ends[:-1] = starts[1:]
    last = np.ones(len(rows), bool)
    last[:-1] = rows[1:] != rows[:-1]
    ends[last] = DAY_END
    first = np.ones(len(rows), bool)
    first[1:] = rows[1:] != rows[:-1]
    seq = np.arange(len(rows)) - np.maximum.accumulate(np.where(first, np.arange(len(rows)), 0))
    s = pa.table(
        {
            "pid": attrs["pid"][rows],
            "seq": seq.astype(np.int16),
            "act": np.array(ACTS)[codes],
            "start": starts.astype(np.int16),
            "end": ends.astype(np.int16),
        }
    )
    return a, s


def generate(
    n: int,
    out_dir: Path,
    *,
    seed: int = 0,
    chunk: int = 1_000_000,
    fmt: str = "parquet",
    verbose: bool = True,
) -> tuple[Path, Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    ext = "parquet" if fmt == "parquet" else "csv"
    a_path, s_path = out_dir / f"attributes.{ext}", out_dir / f"activities.{ext}"
    t0 = time.perf_counter()
    writers: dict[str, object] = {}
    total_segs = 0
    try:
        for lo in range(0, n, chunk):
            m = min(chunk, n - lo)
            rng = np.random.default_rng([seed, lo])
            attrs = gen_attributes(rng, m, lo)
            rows, starts, codes = gen_schedules(rng, attrs)
            a, s = to_tables(attrs, rows, starts, codes)
            total_segs += len(s)
            for key, path, table in (("a", a_path, a), ("s", s_path, s)):
                if key not in writers:
                    writers[key] = (
                        pq.ParquetWriter(path, table.schema, compression="zstd")
                        if fmt == "parquet"
                        else pacsv.CSVWriter(path, table.schema)
                    )
                writers[key].write_table(table)
            if verbose:
                print(
                    f"[{time.perf_counter() - t0:6.1f}s] {lo + m:,} persons, "
                    f"{total_segs:,} segments",
                    file=sys.stderr,
                )
    finally:
        for w in writers.values():
            w.close()
    return a_path, s_path


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        prog="overact-synth",
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "-n", "--persons", type=float, default=1e6, help="number of persons (1e6, 10e6, ...)"
    )
    p.add_argument("-o", "--out-dir", type=Path, required=True)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--format", choices=["parquet", "csv"], default="parquet")
    p.add_argument("--chunk", type=int, default=1_000_000)
    p.add_argument("-q", "--quiet", action="store_true")
    args = p.parse_args(argv)
    a, s = generate(
        int(args.persons),
        args.out_dir,
        seed=args.seed,
        chunk=args.chunk,
        fmt=args.format,
        verbose=not args.quiet,
    )
    print(f"wrote {s} and {a}")
    print(f"pack with: overact-pack {s} -a {a} --seq-col seq -o {args.out_dir}/synth.overact")
    return 0


if __name__ == "__main__":
    sys.exit(main())
