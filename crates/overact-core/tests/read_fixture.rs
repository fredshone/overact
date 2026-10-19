#![allow(clippy::needless_range_loop)]
//! The Rust reader must produce exactly what the Python reader produces.
//! Regenerate the fixture with `uv run --project py python py/scripts/make_rust_fixtures.py`.

use overact_core::Dataset;
use serde_json::Value;

#[test]
fn matches_python_reader() {
    let bytes = std::fs::read("tests/data/synth_small.overact").unwrap();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/synth_small.expected.json").unwrap(),
    )
    .unwrap();
    let ds = Dataset::from_bytes(&bytes).unwrap();
    let ids = expected["ids"].as_array().unwrap();
    let scheds = expected["schedules"].as_array().unwrap();
    assert_eq!(ds.n_persons(), ids.len());
    for p in 0..ds.n_persons() {
        assert_eq!(ds.person_id(p).unwrap(), ids[p].as_str().unwrap());
        let got: Vec<[i64; 3]> = ds
            .person_segments(p)
            .map(|(s, e, c)| [s as i64, e as i64, c as i64])
            .collect();
        let want: Vec<[i64; 3]> = scheds[p]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                let t = t.as_array().unwrap();
                [
                    t[0].as_i64().unwrap(),
                    t[1].as_i64().unwrap(),
                    t[2].as_i64().unwrap(),
                ]
            })
            .collect();
        assert_eq!(got, want, "person {p}");
    }
    let cols = expected["columns"].as_object().unwrap();
    assert_eq!(cols.len(), ds.header.columns.len());
    for (i, meta) in ds.header.columns.iter().enumerate() {
        let col = ds.column(i).unwrap();
        let want = cols[&meta.name].as_array().unwrap();
        for p in 0..ds.n_persons() {
            assert_eq!(
                col.get(p),
                want[p].as_f64(),
                "column {} person {p}",
                meta.name
            );
        }
    }
}

#[test]
fn rejects_bad_files() {
    let bytes = std::fs::read("tests/data/synth_small.overact").unwrap();
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert!(Dataset::from_bytes(&bad).is_err());
    assert!(Dataset::from_bytes(&bytes[..bytes.len() / 2]).is_err());
}

#[test]
fn header_check_enforces_null_sentinels() {
    use overact_core::format::{DType, Header};
    let bytes = std::fs::read("tests/data/synth_small.overact").unwrap();
    let h = Header::from_file_start(&bytes).unwrap();
    let i = h
        .columns
        .iter()
        .position(|c| matches!(c.dtype, DType::I8 | DType::I16 | DType::U8 | DType::U16))
        .expect("an integer column");
    let mut bad = h.clone();
    bad.columns[i].null_value = Some(0.0);
    assert!(Header::parse(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = h.clone();
    bad.time_axis = overact_core::format::TimeAxis {
        start: i32::MIN,
        end: i32::MAX,
        unit: None,
    };
    assert!(Header::parse(&serde_json::to_vec(&bad).unwrap()).is_err());
}

/// M4 acceptance: "working at 09:00 and home at 20:00" through the engine
/// (virtual columns + filter) matches the reference computed in Python from
/// the source table.
#[test]
fn schedule_query_matches_reference() {
    use overact_core::engine::{Engine, ViewSpec};
    use overact_core::features::Feature;
    use overact_core::filter::{ColRef, Condition, Test};

    let bytes = std::fs::read("tests/data/synth_small.overact").unwrap();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/synth_small.expected.json").unwrap(),
    )
    .unwrap();
    let ds = Dataset::from_bytes(&bytes).unwrap();
    let code = |l: &str| {
        ds.header
            .activities
            .iter()
            .position(|a| a.label == l)
            .unwrap() as u8
    };
    let doing = |activity, minute| Condition {
        col: ColRef::Virtual(Feature::DoingAt { activity, minute }),
        test: Test::In {
            values: vec![1],
            null: false,
        },
        negate: false,
    };
    let spec = ViewSpec {
        filter: vec![doing(code("work"), 9 * 60), doing(code("home"), 20 * 60)],
        sort: vec![],
        sample: None,
    };
    let mut engine = Engine::new(ds);
    engine.submit(1, spec).unwrap();
    while engine.has_work() {
        engine.step(1000);
    }
    let rows = &engine.view(1).unwrap().rows;
    let mut got: Vec<String> = rows
        .iter()
        .map(|&p| engine.ds.person_id(p as usize).unwrap().to_string())
        .collect();
    got.sort();
    let want: Vec<String> = expected["work_0900_home_2000"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(!want.is_empty());
    assert_eq!(got, want);
}

#[test]
fn write_round_trips_the_fixture() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/synth_small.overact"
    ))
    .unwrap();
    let ds = overact_core::Dataset::from_bytes(&bytes).unwrap();
    let mut out = Vec::new();
    ds.write(&mut out).unwrap();
    let back = overact_core::Dataset::from_bytes(&out).unwrap();
    assert_eq!(back.offsets, ds.offsets);
    assert_eq!(back.segments, ds.segments);
    assert_eq!(back.person_id(7), ds.person_id(7));
    for i in 0..ds.header.columns.len() {
        let (a, b) = (ds.column(i).unwrap(), back.column(i).unwrap());
        assert_eq!(a.data.bytes(), b.data.bytes());
    }
    assert_eq!(back.header.meta, ds.header.meta);
}
