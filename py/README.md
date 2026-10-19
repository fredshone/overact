# overact (Python tools)

Command-line tools for the [overact](../README.md) viewer:

- `overact-pack`: convert CSV or Parquet activity and attribute tables to `.overact`
  (runs the Rust packer from `crates/overact-ingest`, building it with cargo in a checkout).
- `overact-synth`: generate a synthetic population (up to 10M persons and beyond).
- `overact-validate`: check that a `.overact` file follows [the spec](../docs/FORMAT.md).

Run them with `uv run overact-pack --help` from this directory.
