//! Native benchmark of the hot paths on a real `.overact` file.
//!
//! cargo bench -p overact-core -- path/to/file.overact
//!
//! Prints the PLAN.md budget table. (Single-threaded, like the browser build.)

use overact_core::engine::{Engine, ViewSpec};
use overact_core::filter::{ColRef, Condition, Test};
use overact_core::raster::{Palette, RasterMode, RasterParams};
use overact_core::sort::SortKey;
use overact_core::Dataset;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn run(engine: &mut Engine, view: u32, spec: ViewSpec) -> f64 {
    let t = Instant::now();
    engine.submit(view, spec).unwrap();
    while engine.has_work() {
        engine.step(1 << 22);
    }
    ms(t)
}

fn main() {
    let path = std::env::args()
        .skip(1)
        .find(|a| a.ends_with(".overact"))
        .or_else(|| std::env::var("OVERACT_BENCH_FILE").ok())
        .unwrap_or_else(|| "../../web/public/demo/demo.overact".into());
    let t = Instant::now();
    let bytes = std::fs::read(&path).expect("read file");
    let read_ms = ms(t);
    let t = Instant::now();
    let ds = Dataset::from_bytes(&bytes).expect("parse");
    let parse_ms = ms(t);
    drop(bytes);
    let n = ds.n_persons();
    let col = |name: &str| ds.header.column_index(name).expect("column");
    let (emp, age, income) = (col("employment"), col("age"), col("income"));
    let cats = ds.header.columns[emp].categories.clone().unwrap();
    let employed: Vec<u32> = ["ft-employed", "pt-employed"]
        .iter()
        .map(|c| cats.iter().position(|x| x == c).unwrap() as u32)
        .collect();
    // Hot loops called directly (no job stepping).
    {
        use overact_core::filter::{scan, Bound};
        let c = ds.column(emp).unwrap();
        let cond = Condition {
            col: ColRef::Attr { column: emp },
            test: Test::In {
                values: employed.clone(),
                null: false,
            },
            negate: false,
        };
        let b = Bound::new(&cond, c);
        let t = Instant::now();
        let mut rows = Vec::new();
        scan(&b, 0, n, &mut rows);
        println!("direct: filter scan {:.0} ms ({} rows)", ms(t), rows.len());
        let t = Instant::now();
        let _p = overact_core::profile::profile(&ds, &rows);
        println!("direct: profile {:.0} ms", ms(t));
        let keys = vec![
            SortKey {
                col: ColRef::Attr { column: age },
                desc: false,
                order: None,
            },
            SortKey {
                col: ColRef::Attr { column: income },
                desc: false,
                order: None,
            },
        ];
        let cols = vec![ds.column(age).unwrap(), ds.column(income).unwrap()];
        let t = Instant::now();
        let _s = overact_core::sort::sort_rows(rows, &keys, &cols);
        println!("direct: sort {:.0} ms", ms(t));
    }
    let mut engine = Engine::new(ds);

    let emp_cond = Condition {
        col: ColRef::Attr { column: emp },
        test: Test::In {
            values: employed.clone(),
            null: false,
        },
        negate: false,
    };
    let f1 = run(
        &mut engine,
        1,
        ViewSpec {
            filter: vec![emp_cond.clone()],
            sort: vec![],
            sample: None,
        },
    );
    let age_cond = Condition {
        col: ColRef::Attr { column: age },
        test: Test::Range {
            min: Some(30.0),
            max: Some(60.0),
        },
        negate: false,
    };
    let f2 = run(
        &mut engine,
        2,
        ViewSpec {
            filter: vec![emp_cond.clone(), age_cond],
            sort: vec![],
            sample: None,
        },
    );
    let sort = vec![
        SortKey {
            col: ColRef::Attr { column: age },
            desc: false,
            order: None,
        },
        SortKey {
            col: ColRef::Attr { column: income },
            desc: false,
            order: None,
        },
    ];
    let s = run(
        &mut engine,
        1,
        ViewSpec {
            filter: vec![emp_cond],
            sort,
            sample: None,
        },
    );
    let rows = engine.view(1).unwrap().rows.len();

    let colors: Vec<u32> = (0..32)
        .map(|i| 0x123456u32.wrapping_mul(i + 1) & 0xffffff)
        .collect();
    let pal = Palette::from_srgb(&colors, 0xffffff);
    let (w, h) = (1200u32, 800u32);
    let mut out = vec![0u8; (w * h * 4) as usize];
    let mut times = vec![];
    for i in 0..15 {
        let p = RasterParams {
            width: w,
            height: h,
            row_start: (i * rows / 50) as f64,
            row_end: rows as f64,
            t0: 0.0,
            t1: 1440.0,
            max_samples: 32,
            mode: RasterMode::Mean,
        };
        let t = Instant::now();
        engine.raster(1, &p, &pal, &mut out);
        times.push(ms(t));
    }
    times.sort_by(f64::total_cmp);
    let p = RasterParams {
        width: w,
        height: h,
        row_start: 0.0,
        row_end: rows as f64,
        t0: 0.0,
        t1: 1440.0,
        max_samples: 0,
        mode: RasterMode::Mean,
    };
    let t = Instant::now();
    engine.raster(1, &p, &pal, &mut out);
    let exact = ms(t);

    for (k, ww) in [
        (1u32, 1200u32),
        (1, 100),
        (1, 10),
        (32, 100),
        (32, 1200),
        (64, 1200),
        // While moving: 8 samples, full and half width.
        (8, 1200),
        (8, 600),
    ] {
        let p = RasterParams {
            width: ww,
            height: h,
            row_start: 0.0,
            row_end: rows as f64,
            t0: 0.0,
            t1: 1440.0,
            max_samples: k,
            mode: RasterMode::Mean,
        };
        let mut o = vec![0u8; (ww * h * 4) as usize];
        let t = Instant::now();
        for _ in 0..5 {
            engine.raster(1, &p, &pal, &mut o);
        }
        println!("diag K={k} w={ww}: {:.1} ms", ms(t) / 5.0);
    }
    // Same, but on unsorted rows (sequential memory access).
    engine
        .submit(
            9,
            ViewSpec {
                filter: vec![],
                sort: vec![],
                sample: None,
            },
        )
        .unwrap();
    while engine.has_work() {
        engine.step(1 << 22);
    }
    for (k, ww) in [(32u32, 100u32), (32, 1200)] {
        let p = RasterParams {
            width: ww,
            height: h,
            row_start: 0.0,
            row_end: rows as f64,
            t0: 0.0,
            t1: 1440.0,
            max_samples: k,
            mode: RasterMode::Mean,
        };
        let mut o = vec![0u8; (ww * h * 4) as usize];
        let t = Instant::now();
        for _ in 0..5 {
            engine.raster(9, &p, &pal, &mut o);
        }
        println!("diag unsorted K={k} w={ww}: {:.1} ms", ms(t) / 5.0);
    }
    println!("\n{path}: {n} persons\n");
    println!("| operation | native ms | target |\n|---|---|---|");
    println!("| read file | {read_ms:.0} | |");
    println!("| parse + validate schedules + all columns | {parse_ms:.0} | |");
    println!("| filter, 1 condition (+ profile) | {f1:.0} | 1000 |");
    println!("| filter, 2 conditions (+ profile) | {f2:.0} | 1000 |");
    println!("| 2-key sort of {rows} rows | {s:.0} | 1500 |");
    println!(
        "| raster {w}x{h}, K=32 (median) | {:.1} | 30 |",
        times[times.len() / 2]
    );
    println!("| raster {w}x{h}, exact (all rows) | {exact:.0} | |");
}
