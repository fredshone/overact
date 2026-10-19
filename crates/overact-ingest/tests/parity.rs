//! Golden test: loading the source tables (CSV or Parquet) gives the
//! committed fixture crates/overact-core/tests/data/synth_small.overact
//! (packed with --seq-col seq). If a packing rule changes on purpose,
//! regenerate it with `py/scripts/make_rust_fixtures.py`.

use overact_core::Dataset;
use overact_ingest::{ingest, ByteSource, Config, FileSource, Table};
use std::sync::Arc;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../py/tests/fixtures");

fn table(name: &str) -> Table {
    let src: Arc<dyn ByteSource> = Arc::new(FileSource::open(format!("{FIX}/{name}")).unwrap());
    Table::open(src, name).unwrap()
}

fn packed() -> Dataset {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../overact-core/tests/data/synth_small.overact"
    ))
    .unwrap();
    Dataset::from_bytes(&bytes).unwrap()
}

fn load(sched: &str, attrs: Option<&str>) -> Dataset {
    let cfg = Config {
        seq_col: Some("seq".into()),
        ..Config::default()
    };
    let a = attrs.map(table);
    ingest(&table(sched), a.as_ref(), &cfg, &mut |_, _, _| {}).unwrap()
}

fn assert_same(got: &Dataset, want: &Dataset) {
    let (g, w) = (&got.header, &want.header);
    let labels = |h: &overact_core::Header| {
        h.activities
            .iter()
            .map(|a| a.label.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(labels(g), labels(w));
    assert_eq!(g.n_persons, w.n_persons);
    assert_eq!(g.time_axis.start, w.time_axis.start);
    assert_eq!(g.time_axis.end, w.time_axis.end);
    for p in 0..want.n_persons() {
        assert_eq!(got.person_id(p), want.person_id(p), "person {p}");
        assert_eq!(
            got.person(p),
            want.person(p),
            "schedule of person {p} ({:?})",
            want.person_id(p)
        );
    }
    assert_eq!(got.offsets, want.offsets);
    assert_eq!(g.meta["fixes"], w.meta["fixes"]);
    let names =
        |h: &overact_core::Header| h.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(g), names(w));
    for (i, (gc, wc)) in g.columns.iter().zip(&w.columns).enumerate() {
        assert_eq!(gc.kind, wc.kind, "{}", wc.name);
        assert_eq!(gc.dtype, wc.dtype, "{}", wc.name);
        assert_eq!(gc.null_value, wc.null_value, "{}", wc.name);
        assert_eq!(gc.null_count, wc.null_count, "{}", wc.name);
        assert_eq!(gc.min, wc.min, "{}", wc.name);
        assert_eq!(gc.max, wc.max, "{}", wc.name);
        assert_eq!(gc.categories, wc.categories, "{}", wc.name);
        let (a, b) = (got.column(i).unwrap(), want.column(i).unwrap());
        assert_eq!(a.data.bytes(), b.data.bytes(), "{}", wc.name);
    }
}

#[test]
fn parquet_matches_packer() {
    assert_same(
        &load(
            "synth_small_schedules.parquet",
            Some("synth_small_attributes.parquet"),
        ),
        &packed(),
    );
}

#[test]
fn csv_matches_packer() {
    assert_same(
        &load(
            "synth_small_schedules.csv",
            Some("synth_small_attributes.csv"),
        ),
        &packed(),
    );
}

#[test]
fn mixed_formats_match_packer() {
    assert_same(
        &load(
            "synth_small_schedules.csv",
            Some("synth_small_attributes.parquet"),
        ),
        &packed(),
    );
}

#[test]
fn inspect_shows_columns_types_and_preview() {
    for name in ["synth_small_schedules.parquet", "synth_small_schedules.csv"] {
        let info = overact_ingest::inspect(&table(name), 3).unwrap();
        let cols: Vec<_> = info.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            cols,
            ["pid", "seq", "act", "zone", "start", "end"],
            "{name}"
        );
        assert_eq!(
            info.columns[4].ty,
            overact_ingest::table::ColType::Int,
            "{name}"
        );
        assert_eq!(info.preview.len(), 3);
        assert_eq!(info.preview[0][4].as_deref(), Some("0"), "{name}");
    }
}
