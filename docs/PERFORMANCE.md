# Performance

Measured with the tools in this repository, so anyone can rerun them:

```sh
# synthetic 10M dataset (about 1.5 min) and pack it (about 40 s)
uv run --project py overact-synth -n 10e6 -o scratch/s10m
uv run --project py overact-pack scratch/s10m/activities.parquet \
  -a scratch/s10m/attributes.parquet --seq-col seq -o scratch/s10m/synth.overact

# native benchmark (single-threaded, then with rayon)
cargo bench -p overact-core --bench engine -- scratch/s10m/synth.overact
cargo bench -p overact-core --features parallel --bench engine -- scratch/s10m/synth.overact

# browser benchmark (start `npx vite` in web/ first)
cd web && npm run bench -- ../scratch/s10m/synth.overact --threads=0
cd web && npm run bench -- ../scratch/s10m/synth.overact --partB=0
```

## Machine and data

- **CPU:** Intel Core i5-8350U, 4 cores and 8 threads. This is a 2017 laptop
  CPU running the powersave governor. It is slower than the "modern laptop"
  the budgets assume.
- **RAM:** 16 GB. **OS:** Linux. **Browser:** headless Chromium 156 (Playwright).
- **Load:** the machine was shared with other desktop applications. Early runs
  were up to 3× slower while a busy browser used 100% CPU; the tables below
  come from a quiet period.
- **Data:** synthetic, 10,000,000 persons and 68,954,993 segments (6.9 per
  person), 12 attribute columns, 467 MB file.

## Budgets at 10M persons (browser, app path through the data worker)

| Operation | Target | 1 thread | 8 threads (+SIMD) |
|---|---|---|---|
| Load schedule arrays (file served locally over HTTP) | < 10 s | 1.6 s | 1.7 s |
| First use of a 10M-row column (load + filter) | < 1 s | 264 ms | 256 ms |
| Filter, 1 condition: rows ready (preview) | < 300 ms | 60 ms | 98 ms |
| Filter, 1 condition: profile ready (complete) | < 1 s | 297 ms | 137 ms |
| Filter, 2 conditions: rows / profile ready | < 300 ms / < 1 s | 54 / 138 ms | 68 / 173 ms |
| 2-key sort (age, income) of 4.7M rows | < 1.5 s | 233 ms | 146 ms |
| Re-raster 1200×800, 32 samples/row (incl. canvas upload) | < 30 ms | **35 ms** (raster 34) | 24 ms (raster 13) |
| Re-raster while moving, 8 samples/row | < 30 ms | 30 ms (raster 29) | 18 ms (raster 7) |
| WASM memory, dataset + 4 views | < 1.5 GB | 623 MB | 653 MB |

All targets are met on this machine except one: single-threaded re-raster
at the full 32 samples per pixel row takes 35 ms. What the viewer does about it:

- **While scrolling, zooming or dragging** it draws with 8 samples per row
  (30 ms here), then redraws at 32 when you stop, then runs an exact pass.
- **The exact pass** is drawn in horizontal bands, so it never blocks the
  worker for long. At 10M it takes about 2 s single-threaded and 0.3 s with
  threads.
- **With threads** (any browser where the page is cross-origin isolated,
  which `coi-serviceworker` arranges on GitHub Pages) the raster takes 13 ms.

- **Single-threaded, while moving** it also draws at half the horizontal
  resolution (the canvas is stretched back), since the per-pixel colour pass
  is a large part of the cost at 1200 px wide.

Half width while moving, measured 2026-10-18 (same machine, single thread,
10M, browser; the machine was busier than for the table above, so compare
within this table):

| Raster 800 px tall, K=8 (incl. canvas upload, median) | ms |
|---|---|
| 1200 px wide | 32 |
| 600 px wide (what the viewer now draws while moving) | 25 |

Natively the same change is 8.9 → 7.0 ms. In that run, WASM memory after 4
views was 548 MB (623 MB in the table above). View row lists are now shared
between views and jobs instead of copied, so changing the sort or sample no
longer copies the filter result. WASM memory never shrinks, so these peaks
are what the tab keeps.

## Is WASM worth it? (same thread, same data, no job overhead)

| Hot loop, 10M persons | WASM | plain TypeScript |
|---|---|---|
| Filter scan, 1 condition (10M rows) | 69 ms | 109 ms |
| Profile (4.7M persons) | 110 ms | 227 ms |
| 2-key radix sort (4.7M rows) | 240 ms | 257 ms |
| Raster 1200×800, K=32 | 23 ms | 38 ms (no sRGB conversion) |

Single-threaded WASM is 1.1× to 2× faster than equivalent TypeScript. The
sort is memory-bound, so the two are close. The bigger gains come from what
WASM enables:
- Threads (below).
- The whole dataset as compact typed arrays inside one linear memory.
- Sharing the core with a native build.

## Threads (M5)

**Native**, single-threaded vs rayon with 8 threads
(`cargo bench -p overact-core [--features parallel]`, 10M):

| Operation | 1 thread | 8 threads | Speedup |
|---|---|---|---|
| Filter scan (10M rows) | 31 ms | 19 ms | 1.6× |
| Profile (4.7M persons) | 98 ms | 25 ms | 3.9× |
| 2-key sort (4.7M rows) | 181 ms | 66 ms | 2.7× |
| Raster 1200×800, K=32 | 18.6 ms | 3.1 ms | 6.0× |
| Raster 1200×800, exact (4.7M rows) | 1982 ms | 297 ms | 6.7× |

**Browser:** the raster speeds up 2.6× (34 → 13 ms) and the profile about 2×.
Canvas upload (copying from shared memory, then `putImageData`) adds about
11 ms per frame in the threaded build. Small jobs (a single filter at 10M) are
no faster, because they run in 12 ms steps and dispatching work to threads at
each step has a cost.

The parallel paths give results identical to the single-threaded ones. The
`tests_parallel` test runs every operation on 1M persons in a 1-thread and a
4-thread pool and compares rows, profiles and pixels exactly (CI:
`cargo test -p overact-core --features parallel --release`).

## SIMD

The threaded build is compiled with `simd128`. Measured on the
single-threaded hot loops, auto-vectorised SIMD changes nothing:

| Hot loop (single thread) | without simd128 | with simd128 |
|---|---|---|
| Filter scan | 69 ms | 68 ms |
| Profile | 110 ms | 116 ms |
| Radix sort | 240 ms | 236 ms |
| Raster K=32 | 23 ms | 22 ms |

These loops are dominated by gathers, scatters and data-dependent memory
access, which SIMD does not help. Hand-written SIMD was therefore not added.
The flag stays on the threaded build because it costs nothing and every
browser that supports WASM threads also supports SIMD.

## Loading tables in the browser (`overact-ingest`)

Synthetic data from `overact-synth`, same machine, Chrome via Playwright,
threaded build (ingest itself is single-threaded). Schedules and attributes
both given, `seq` as the tie-breaker.

| Input | Persons / rows | Browser | WASM memory | Native |
|---|---|---|---|---|
| Parquet (zstd) | 1M / 6.9M | 3.8 s | 194 MB | 2.7 s |
| CSV | 1M / 6.9M | 5.2 s | 130 MB | 3.4 s |
| Parquet (zstd) | 10M / 69M | 51 s | 1.19 GB | 29 s |
| CSV (2.7 GB) | 10M / 69M | 66 s | 1.24 GB | 43 s |
| Parquet, rows shuffled (buffered path) | 10M / 69M | 185 s | 2.4 GB | 171 s |

Native timings: `cargo run --release -p overact-ingest --bin overact-pack -- ACTS -a ATTRS --seq-col seq -o out.overact`.
`overact-pack` is this same code (see below).

What mattered (10M Parquet, native):

- Most of the time is Parquet decoding (about 7 s for the activity columns,
  4 s for attributes) and one id lookup per person. A lookup is about three
  cache misses (hash slot, offsets, bytes), about 300 ns here.
- Checking first whether the next person is the next index (tables sorted
  alike) skips most lookups: activities 24.5 s → 17 s.
- Sizing the id table and segment store from the first batch avoids rehashing
  and Vec doubling (which briefly needs 3× the final size): 1.64 GB → 1.19 GB
  of WASM memory in the browser. WASM memory never shrinks, so the peak is
  what the tab keeps.
- Recoding segments in place when persons are already in final order avoids
  a second copy.
- Linear scans beat hashing for activity labels (≤ 32) and for categories
  while a column has ≤ 32 values.

Rows not grouped by person fall back to two passes (count, then scatter into
per-person slots). That path is about 4× slower and needs about 8–12 bytes per
row, because every row costs a random hash lookup and a random write.

WASM size: the Parquet reader (with zstd, snappy, gzip and lz4) takes the
single-threaded module from 0.5 MB to 2.3 MB (0.7 MB gzipped). gzip and lz4
cost about 120 kB and 30 kB; zstd and the Parquet reader account for the rest.

## Converter (`overact-pack`)

`overact-pack` is the Rust binary in `crates/overact-ingest` (the Python
`overact-pack` command runs it). Measured 2026-10-18:

| 10M persons (69M rows, Parquet input, `--seq-col seq`) | Time | Peak RAM |
|---|---|---|
| Rust `overact-pack` | 30 s | 0.9 GB |
| Former Python/polars packer | 41 s | 2.3 GB |

The Rust output has the same bytes in every blob (offsets, segments, ids and
all 12 columns) as the file the Python packer wrote; only the free-form
`meta` differs.

`overact-synth -n 10e6` takes about 86 s and peaks at 2 GB.
