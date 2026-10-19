//! The parallel paths must give exactly the single-threaded results. Runs
//! everything in a 1-thread pool (sequential code paths) and in the default
//! pool, on data large enough to take the parallel paths.

use crate::column::ColumnData;
use crate::engine::{Engine, ViewSpec};
use crate::features::Feature;
use crate::filter::{ColRef, Condition, Test};
use crate::raster::{Palette, RasterMode, RasterParams};
use crate::sort::SortKey;
use crate::testutil::dataset_with_columns;

fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

fn big_engine() -> Engine {
    let n = 1_000_000;
    let mut s = 42u64;
    let sched: Vec<Vec<(u16, u8)>> = (0..n)
        .map(|_| {
            let k = (lcg(&mut s) % 6) as usize;
            let mut cuts: Vec<u16> = (0..k).map(|_| (lcg(&mut s) % 1439 + 1) as u16).collect();
            cuts.sort();
            cuts.dedup();
            let mut out = vec![(0u16, (lcg(&mut s) % 5) as u8)];
            for c in cuts {
                let mut code = (lcg(&mut s) % 5) as u8;
                if code == out.last().unwrap().1 {
                    code = (code + 1) % 5;
                }
                out.push((c, code));
            }
            out
        })
        .collect();
    let age: Vec<i8> = (0..n)
        .map(|_| {
            if lcg(&mut s).is_multiple_of(50) {
                i8::MIN
            } else {
                (lcg(&mut s) % 90) as i8
            }
        })
        .collect();
    let inc: Vec<f32> = (0..n)
        .map(|_| {
            if lcg(&mut s).is_multiple_of(30) {
                f32::NAN
            } else {
                (lcg(&mut s) % 1000) as f32 * 37.5
            }
        })
        .collect();
    let cat: Vec<u8> = (0..n).map(|_| (lcg(&mut s) % 4) as u8).collect();
    let ds = dataset_with_columns(
        1440,
        5,
        &sched,
        vec![
            ("age", "numeric", None, ColumnData::I8(age)),
            ("income", "numeric", None, ColumnData::F32(inc)),
            (
                "cat",
                "categorical",
                Some(vec!["a".into(), "b".into(), "c".into(), "d".into()]),
                ColumnData::U8(cat),
            ),
        ],
    );
    Engine::new(ds)
}

fn run(engine: &mut Engine, view: u32, spec: ViewSpec) {
    engine.submit(view, spec).unwrap();
    while engine.has_work() {
        engine.step(1 << 30);
    }
}

#[test]
fn parallel_matches_sequential() {
    let spec = ViewSpec {
        filter: vec![
            Condition {
                col: ColRef::Attr { column: 2 },
                test: Test::In {
                    values: vec![0, 2, 3],
                    null: false,
                },
                negate: false,
            },
            Condition {
                col: ColRef::Attr { column: 0 },
                test: Test::Range {
                    min: Some(10.0),
                    max: Some(80.0),
                },
                negate: false,
            },
            Condition {
                col: ColRef::Virtual(Feature::DoingDuring {
                    activity: 1,
                    start: 400,
                    end: 700,
                    all: false,
                }),
                test: Test::In {
                    values: vec![1],
                    null: false,
                },
                negate: true,
            },
        ],
        sort: vec![
            SortKey {
                col: ColRef::Virtual(Feature::minutes(2)),
                desc: true,
                order: None,
            },
            SortKey {
                col: ColRef::Attr { column: 1 },
                desc: false,
                order: None,
            },
            SortKey {
                col: ColRef::Attr { column: 2 },
                desc: false,
                order: Some(vec![3, 1, 0, 2]),
            },
        ],
        sample: None,
    };
    let pal = Palette::from_srgb(
        &[0xe69f00, 0x56b4e9, 0x009e73, 0xf0e442, 0x0072b2],
        0x202020,
    );
    let draw = |engine: &mut Engine, mode| {
        let p = RasterParams {
            width: 333,
            height: 97,
            row_start: 12.5,
            row_end: 150_000.0,
            t0: -30.0,
            t1: 1500.0,
            max_samples: 0,
            mode,
        };
        let mut out = vec![0u8; 333 * 97 * 4];
        engine.raster(1, &p, &pal, &mut out);
        out
    };
    let compute = || {
        let mut e = big_engine();
        run(&mut e, 1, spec.clone());
        let v = e.view(1).unwrap();
        let (rows, profile) = (v.rows.clone(), v.profile.clone().unwrap());
        (
            rows,
            profile,
            draw(&mut e, RasterMode::Mean),
            draw(&mut e, RasterMode::Mode),
            draw(&mut e, RasterMode::Sample),
        )
    };
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let four = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    let seq = one.install(compute);
    let par = four.install(compute);
    assert!(
        seq.0.len() >= 1 << 18,
        "test should exercise the parallel paths ({} rows)",
        seq.0.len()
    );
    assert_eq!(seq.0, par.0, "rows");
    assert_eq!(seq.1, par.1, "profile");
    assert_eq!(seq.2, par.2, "mean raster");
    assert_eq!(seq.3, par.3, "mode raster");
}
