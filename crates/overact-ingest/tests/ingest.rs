//! Behaviour of `ingest` on small hand-made tables.

use overact_core::Dataset;
use overact_ingest::{ingest, ByteSource, Config, Table, TimeUnit};
use std::sync::Arc;

fn table(name: &str, text: &str) -> Table {
    let src: Arc<dyn ByteSource> = Arc::new(text.as_bytes().to_vec());
    Table::open(src, name).unwrap()
}

fn err(r: Result<Dataset, String>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e,
    }
}

fn run(sched: &str, attrs: Option<&str>, cfg: &Config) -> Result<Dataset, String> {
    let a = attrs.map(|t| table("a.csv", t));
    ingest(&table("s.csv", sched), a.as_ref(), cfg, &mut |_, _, _| {}).map_err(|e| e.0)
}

/// (person id, [(start, end, label)]) per person.
type Schedules = Vec<(String, Vec<(u16, u16, String)>)>;

fn schedules(ds: &Dataset) -> Schedules {
    (0..ds.n_persons())
        .map(|p| {
            let segs = ds
                .person_segments(p)
                .map(|(s, e, c)| (s, e, ds.header.activities[c as usize].label.clone()))
                .collect();
            (ds.person_id(p).unwrap().to_string(), segs)
        })
        .collect()
}

const GROUPED: &str =
    "pid,act,start,end\nb,home,0,480\nb,work,480,1440\na,home,0,1440\nc,shop,600,660\n";
const SHUFFLED: &str =
    "pid,act,start,end\nb,work,480,1440\na,home,0,1440\nc,shop,600,660\nb,home,0,480\n";

#[test]
fn grouped_and_shuffled_rows_agree() {
    let cfg = Config::default();
    let g = run(GROUPED, None, &cfg).unwrap();
    let s = run(SHUFFLED, None, &cfg).unwrap();
    assert_eq!(g.header.meta["mode"], "grouped");
    assert_eq!(s.header.meta["mode"], "buffered");
    assert_eq!(schedules(&g), schedules(&s));
    assert_eq!(g.header.meta["fixes"], s.header.meta["fixes"]);
    // No attributes: persons sorted by id; activities by total time.
    let sch = schedules(&g);
    assert_eq!(
        sch.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    let labels: Vec<_> = g
        .header
        .activities
        .iter()
        .map(|a| a.label.as_str())
        .collect();
    assert_eq!(labels, ["home", "work", "shop", "unknown"]);
    assert_eq!(
        sch[2].1,
        [
            (0, 600, "unknown".into()),
            (600, 660, "shop".into()),
            (660, 1440, "unknown".into())
        ]
    );
}

#[test]
fn attributes_set_person_order_and_extras_follow() {
    let attrs = "pid,age,emp,ok\nc,30,yes,true\nz,40,no,false\nb,,yes,\n";
    let ds = run(GROUPED, Some(attrs), &Config::default()).unwrap();
    let ids: Vec<_> = (0..ds.n_persons())
        .map(|p| ds.person_id(p).unwrap())
        .collect();
    assert_eq!(ids, ["c", "z", "b", "a"]);
    // z has no activities: a whole day of unknown.
    assert_eq!(schedules(&ds)[1].1, [(0, 1440, "unknown".into())]);
    assert_eq!(ds.header.meta["fixes"]["person_filled"], 1);
    let age = ds.column(0).unwrap();
    assert_eq!(
        (0..4).map(|p| age.get(p)).collect::<Vec<_>>(),
        [Some(30.0), Some(40.0), None, None]
    );
    assert_eq!(
        ds.header.columns[1].categories.as_deref().unwrap(),
        ["no", "yes"]
    );
    assert_eq!(
        ds.header.columns[2].kind,
        overact_core::format::ColumnKind::Boolean
    );
    let w = ds.header.meta["warnings"].as_array().unwrap();
    assert!(w[0]
        .as_str()
        .unwrap()
        .contains("1 persons have activities but no attributes"));
}

#[test]
fn duration_units_and_axis() {
    let sched = "id\tlabel\tfrom\tdur\nx\tsleep\t4\t3.5\nx\twork\t7.5\t20.5\n";
    let cfg = Config {
        id_col: "id".into(),
        act_col: "label".into(),
        start_col: "from".into(),
        duration_col: Some("dur".into()),
        time_unit: TimeUnit::Hour,
        axis: (240, 1680),
        ..Config::default()
    };
    let src: Arc<dyn ByteSource> = Arc::new(sched.as_bytes().to_vec());
    let t = Table::open(src, "s.tsv").unwrap();
    let ds = ingest(&t, None, &cfg, &mut |_, _, _| {}).unwrap();
    assert_eq!(
        schedules(&ds)[0].1,
        [(0, 210, "sleep".into()), (210, 1440, "work".into())]
    );
}

#[test]
fn errors() {
    let cfg = Config::default();
    let dup = "pid,age\na,1\na,2\n";
    assert!(err(run(GROUPED, Some(dup), &cfg)).contains("duplicate person id"));
    let many: String = std::iter::once("pid,act,start,end\n".to_string())
        .chain((0..33).map(|i| format!("p,a{i},{i},{}\n", i + 1)))
        .collect();
    assert!(err(run(&many, None, &cfg)).contains("more than 32 activity labels"));
    assert!(err(run("pid,act,start,end\na,x,zero,10\n", None, &cfg)).contains("not a number"));
    // Like overact-pack, which cannot cast NaN or infinity to minutes.
    assert!(err(run("pid,act,start,end\na,x,NaN,10\n", None, &cfg)).contains("not a number"));
    assert!(err(run("pid,act,start,end\na,x,0,inf\n", None, &cfg)).contains("not a number"));
    assert!(err(run(
        GROUPED,
        None,
        &Config {
            act_col: "nope".into(),
            ..cfg.clone()
        }
    ))
    .contains("no column \"nope\""));
}

#[test]
fn csv_type_inference_restarts_on_late_text() {
    let mut attrs = String::from("pid,code\n");
    for i in 0..10 {
        attrs.push_str(&format!("p{i},{i}\n"));
    }
    attrs.push_str("p10,x7\n");
    let sched = "pid,act,start,end\np0,home,0,1440\n";
    let cfg = Config {
        infer_rows: 5,
        ..Config::default()
    };
    let ds = run(sched, Some(&attrs), &cfg).unwrap();
    let c = &ds.header.columns[0];
    assert_eq!(c.kind, overact_core::format::ColumnKind::Categorical);
    assert_eq!(c.categories.as_ref().unwrap().last().unwrap(), "x7");
    // Floats after integers widen in place.
    let attrs = "pid,v\np0,1\np1,2\np2,2.5\n";
    let ds = run(
        sched,
        Some(attrs),
        &Config {
            infer_rows: 2,
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(ds.header.columns[0].dtype, overact_core::format::DType::F32);
}

#[test]
fn subsets_are_deterministic_and_nested() {
    let mut sched = String::from("pid,act,start,end\n");
    for i in 0..4000 {
        sched.push_str(&format!("p{i},home,0,1440\n"));
    }
    let ids = |f: f64| {
        let ds = run(
            &sched,
            None,
            &Config {
                fraction: f,
                ..Config::default()
            },
        )
        .unwrap();
        (0..ds.n_persons())
            .map(|p| ds.person_id(p).unwrap().to_string())
            .collect::<std::collections::HashSet<_>>()
    };
    let (small, big) = (ids(0.1), ids(0.25));
    assert!(small.is_subset(&big));
    assert_eq!(small, ids(0.1));
    assert!((300..500).contains(&small.len()), "{}", small.len());
}

#[test]
fn persons_with_only_incomplete_rows_are_dropped_in_both_modes() {
    // "d" has a single row with a missing start; "e" has rows, but all are
    // empty (dropped by cleaning), so "e" stays as a day of `unknown`.
    let grouped = "pid,act,start,end\na,home,0,700\na,work,700,1440\nd,home,,1440\ne,home,5,5\nb,home,0,1440\n";
    let shuffled = "pid,act,start,end\na,home,0,700\nd,home,,1440\nb,home,0,1440\ne,home,5,5\na,work,700,1440\n";
    let attrs = "pid,age\nb,30\na,40\n";
    for attrs in [None, Some(attrs)] {
        let cfg = Config::default();
        let g = run(grouped, attrs, &cfg).unwrap();
        let s = run(shuffled, attrs, &cfg).unwrap();
        assert_eq!(g.header.meta["mode"], "grouped");
        assert_eq!(s.header.meta["mode"], "buffered");
        assert_eq!(schedules(&g), schedules(&s));
        assert_eq!(g.header.meta["fixes"], s.header.meta["fixes"]);
        assert_eq!(g.header.meta["fix_examples"], s.header.meta["fix_examples"]);
        let ids: Vec<String> = schedules(&s).into_iter().map(|x| x.0).collect();
        let want: &[&str] = if attrs.is_some() {
            &["b", "a", "e"]
        } else {
            &["a", "b", "e"]
        };
        assert_eq!(ids, want);
        assert_eq!(s.header.meta["fixes"]["dropped_missing"], 1);
    }
}
