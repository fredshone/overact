"""overact-pack: convert CSV or Parquet activity/attribute tables to `.overact`.

The packer is written in Rust (`crates/overact-ingest`, binary `overact-pack`)
so that the browser's direct table loading and the command line share one
implementation. This module finds or builds that binary and runs it:

1. `$OVERACT_PACK_BIN`, if set;
2. in a source checkout, `cargo build --release` of the binary (a no-op
   when it is up to date), or the last build if cargo is not on `PATH`;
3. an `overact-pack` executable on `PATH` that is not this wrapper.

`pack()` keeps the Python API of the old pure-Python packer.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from collections import Counter
from dataclasses import dataclass
from functools import cache
from pathlib import Path
from typing import Any

from .format import read_header

ROOT = Path(__file__).resolve().parents[3]

FIX_DESCRIPTIONS = {
    "dropped_missing": "rows with a missing id, activity, start or end were dropped",
    "dropped_empty": "rows with end <= start were dropped",
    "dropped_outside": "rows entirely outside the time axis were dropped",
    "clipped": "rows partly outside the time axis were clipped",
    "overlap_trimmed": "rows overlapping an earlier row had their start moved later",
    "overlap_dropped": "rows fully covered by an earlier row were dropped",
    "gap_filled": "gaps (including at the start or end of the day) were filled with 'unknown'",
    "person_filled": "persons with no activities got a whole day of 'unknown'",
}


@dataclass
class PackResult:
    header: dict[str, Any]
    fixes: Counter
    warnings: list[str]
    extra_persons: int
    seconds: float


def _is_native(path: str) -> bool:
    """True for a compiled executable (not a Python console script like this one)."""
    try:
        with open(path, "rb") as f:
            return f.read(2) != b"#!"
    except OSError:
        return False


@cache
def binary() -> str:
    """Path of the Rust `overact-pack` executable (built on first use in a checkout)."""
    env = os.environ.get("OVERACT_PACK_BIN")
    if env:
        return env
    if (ROOT / "crates" / "overact-ingest").is_dir() and shutil.which("cargo"):
        cmd = ["cargo", "build", "--release", "--quiet", "--message-format=json"]
        cmd += ["-p", "overact-ingest", "--bin", "overact-pack"]
        res = subprocess.run(cmd, check=False, cwd=ROOT, capture_output=True, text=True)
        if res.returncode != 0:
            raise RuntimeError(f"building overact-pack failed:\n{res.stderr}")
        for line in res.stdout.splitlines():
            msg = json.loads(line)
            if msg.get("executable") and msg.get("target", {}).get("name") == "overact-pack":
                return msg["executable"]
    built = ROOT / "target" / "release" / "overact-pack"
    if built.is_file():
        return str(built)
    for d in os.environ.get("PATH", "").split(os.pathsep):
        exe = shutil.which("overact-pack", path=d)
        if exe and _is_native(exe):
            return exe
    raise RuntimeError(
        "overact-pack (Rust) not found: run `cargo build --release -p overact-ingest` "
        "in the repository, or set OVERACT_PACK_BIN"
    )


def _args(
    activities: Path,
    attributes: Path | None,
    out: Path,
    *,
    id_col: str = "pid",
    act_col: str = "act",
    start_col: str = "start",
    end_col: str | None = "end",
    duration_col: str | None = None,
    seq_col: str | None = None,
    attr_id_col: str | None = None,
    axis: tuple[int, int] = (0, 1440),
    time_unit: str = "minute",
    columns: list[str] | None = None,
    exclude: list[str] | None = None,
    f64_cols: list[str] | None = None,
    store_ids: bool = True,
    infer_rows: int | None = 100_000,
    report_path: Path | None = None,
    verbose: bool = True,
) -> list[str]:
    a = [str(activities), "-o", str(out), "--id-col", id_col, "--act-col", act_col]
    a += ["--start-col", start_col, "--axis", f"{axis[0]},{axis[1]}", "--time-unit", time_unit]
    opt = {
        "-a": attributes,
        "--end-col": end_col if duration_col is None else None,
        "--duration-col": duration_col,
        "--seq-col": seq_col,
        "--attr-id-col": attr_id_col,
        "--columns": ",".join(columns) if columns else None,
        "--exclude": ",".join(exclude) if exclude else None,
        "--f64": ",".join(f64_cols) if f64_cols else None,
        "--infer-rows": infer_rows,
        "--report": report_path,
    }
    for flag, v in opt.items():
        if v is not None:
            a += [flag, str(v)]
    if not store_ids:
        a.append("--no-ids")
    if not verbose:
        a.append("-q")
    return a


def pack(
    activities: Path,
    attributes: Path | None,
    out: Path,
    *,
    end_col: str | None = "end",
    duration_col: str | None = None,
    bucket_persons: int | None = None,
    tmp_dir: Path | None = None,
    verbose: bool = True,
    **kw: Any,
) -> PackResult:
    """Packs tables into `out` with the Rust packer. Raises ValueError on bad input.

    `bucket_persons` and `tmp_dir` belonged to the old Python packer and are ignored.
    """
    del bucket_persons, tmp_dir
    if (end_col is None) == (duration_col is None):
        raise ValueError("give exactly one of end_col or duration_col")
    t0 = time.perf_counter()
    argv = _args(
        activities,
        attributes,
        out,
        end_col=end_col,
        duration_col=duration_col,
        verbose=verbose,
        **kw,
    )
    res = subprocess.run(
        [binary(), *argv],
        check=False,
        stdout=subprocess.PIPE,
        stderr=None if verbose else subprocess.PIPE,
        text=True,
    )
    if res.returncode != 0:
        err = (res.stderr or "").strip().removeprefix("overact-pack: error: ")
        raise ValueError(err or f"overact-pack failed ({res.returncode})")
    if verbose:
        print(res.stdout, end="")
    header = read_header(out)
    meta = header.get("meta", {})
    return PackResult(
        header,
        Counter(meta.get("fixes", {})),
        list(meta.get("warnings", [])),
        int(meta.get("extra_persons", 0)),
        time.perf_counter() - t0,
    )


def main(argv: list[str] | None = None) -> int:
    """`overact-pack` on the command line: runs the Rust binary with the same arguments."""
    args = sys.argv[1:] if argv is None else argv
    try:
        exe = binary()
    except RuntimeError as e:
        print(f"overact-pack: error: {e}", file=sys.stderr)
        return 1
    return subprocess.run([exe, *args], check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
