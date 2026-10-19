//! Property tests: engine results vs naive reference implementations.
#![allow(clippy::needless_range_loop)]

use crate::column::ColumnData;
use crate::engine::{Engine, ViewSpec};
use crate::features::{Feature, Measure};
use crate::filter::{ColRef, Condition, Test};
use crate::profile::profile;
use crate::raster::tests::schedules_strategy;
use crate::sample::{rank, Sample};
use crate::sort::SortKey;
use crate::testutil::dataset_with_columns;
use proptest::prelude::*;
use std::cmp::Ordering;

const N_COLS: usize = 5;

fn columns_strategy(n: usize) -> impl Strategy<Value = Vec<crate::testutil::TestColumn>> {
    (
        proptest::collection::vec(prop_oneof![Just(-128i8), -5i8..5], n),
        proptest::collection::vec(prop_oneof![Just(f32::NAN), Just(-0.0f32), -3.0f32..3.0], n),
        proptest::collection::vec(prop_oneof![Just(255u8), 0u8..4], n),
        proptest::collection::vec(prop_oneof![Just(255u8), 0u8..2], n),
        proptest::collection::vec(
            prop_oneof![Just(f64::NAN), -1e300f64..1e300, -2.0f64..2.0],
            n,
        ),
    )
        .prop_map(|(a, b, c, d, e)| {
            vec![
                ("i8", "numeric", None, ColumnData::I8(a)),
                ("f32", "numeric", None, ColumnData::F32(b)),
                (
                    "cat",
                    "categorical",
                    Some(vec!["w".into(), "x".into(), "y".into(), "z".into()]),
                    ColumnData::U8(c),
                ),
                ("bool", "boolean", None, ColumnData::U8(d)),
                ("f64", "numeric", None, ColumnData::F64(e)),
            ]
        })
}

fn colref_strategy() -> impl Strategy<Value = ColRef> {
    prop_oneof![
        (0usize..N_COLS).prop_map(|column| ColRef::Attr { column }),
        (0u8..3).prop_map(|activity| ColRef::Virtual(Feature::minutes(activity))),
        (0u8..3).prop_map(|activity| ColRef::Virtual(Feature::first_start(activity))),
        (0u8..3, 0u16..300)
            .prop_map(|(activity, minute)| ColRef::Virtual(Feature::DoingAt { activity, minute })),
        (
            prop_oneof![
                Just(Measure::Count),
                Just(Measure::Start),
                Just(Measure::Duration),
                Just(Measure::End)
            ],
            proptest::option::of(0u8..3),
            any::<bool>(),
            -3i16..4,
        )
            .prop_map(
                |(measure, activity, not, nth)| ColRef::Virtual(Feature::Measure {
                    measure,
                    activity,
                    not,
                    nth
                })
            ),
    ]
}

fn condition_strategy() -> impl Strategy<Value = Condition> {
    let test = prop_oneof![
        (
            proptest::option::of(-4.0f64..4.0),
            proptest::option::of(-4.0f64..300.0)
        )
            .prop_map(|(min, max)| Test::Range { min, max }),
        (proptest::collection::vec(0u32..5, 0..4), any::<bool>())
            .prop_map(|(values, null)| Test::In { values, null }),
        Just(Test::IsNull),
    ];
    (colref_strategy(), test, any::<bool>()).prop_map(|(col, test, negate)| Condition {
        col,
        test,
        negate,
    })
}

fn sortkey_strategy() -> impl Strategy<Value = SortKey> {
    (colref_strategy(), any::<bool>(), any::<bool>()).prop_map(|(col, desc, custom)| {
        let order = (custom && matches!(col, ColRef::Attr { column: 2 })).then(|| vec![2, 0, 3, 1]);
        SortKey { col, desc, order }
    })
}

/// Naive value lookup straight from the schedules / raw data.
fn naive_value(ds: &crate::Dataset, col: &ColRef, p: usize) -> Option<f64> {
    match col {
        ColRef::Attr { column } => ds.column(*column).unwrap().get(p),
        ColRef::Virtual(f) => {
            let segs: Vec<(u16, u16, u8)> = ds.person_segments(p).collect();
            match *f {
                Feature::DoingAt { activity, minute } => Some(
                    segs.iter()
                        .any(|s| s.0 <= minute && minute < s.1 && s.2 == activity)
                        as u8 as f64,
                ),
                Feature::Measure {
                    measure,
                    activity,
                    not,
                    nth,
                } => {
                    let eps: Vec<_> = segs
                        .iter()
                        .filter(|s| match activity {
                            None => true,
                            Some(a) => (s.2 == a) != not,
                        })
                        .collect();
                    let pick = |k: i16| -> Option<&(u16, u16, u8)> {
                        let i = if k > 0 {
                            k as i64 - 1
                        } else {
                            eps.len() as i64 + k as i64
                        };
                        (0..eps.len() as i64).contains(&i).then(|| eps[i as usize])
                    };
                    match (measure, nth) {
                        (Measure::Count, 0) => Some(eps.len() as f64),
                        (Measure::Count, k) => Some(pick(k).is_some() as u8 as f64),
                        (Measure::Duration, 0) => {
                            Some(eps.iter().map(|s| (s.1 - s.0) as f64).sum())
                        }
                        (Measure::Duration, k) => pick(k).map(|s| (s.1 - s.0) as f64),
                        (Measure::Start, k) => pick(if k == 0 { 1 } else { k }).map(|s| s.0 as f64),
                        (Measure::End, k) => pick(if k == 0 { -1 } else { k }).map(|s| s.1 as f64),
                    }
                }
                _ => unreachable!(),
            }
        }
    }
}

fn naive_test(c: &Condition, v: Option<f64>) -> bool {
    let r = match (&c.test, v) {
        (Test::IsNull, v) => v.is_none(),
        (Test::Range { .. }, None) => false,
        (Test::Range { min, max }, Some(v)) => {
            min.is_none_or(|m| v >= m) && max.is_none_or(|m| v <= m)
        }
        (Test::In { null, .. }, None) => *null,
        (Test::In { values, .. }, Some(v)) => values.iter().any(|&x| x as f64 == v),
    };
    r != c.negate
}

fn naive_cmp(ds: &crate::Dataset, keys: &[SortKey], a: u32, b: u32) -> Ordering {
    for k in keys {
        let rank = |p: u32| {
            naive_value(ds, &k.col, p as usize).map(|v| match &k.order {
                Some(o) => o[v as usize] as f64,
                None => v,
            })
        };
        let o = match (rank(a), rank(b)) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => {
                let o = x.partial_cmp(&y).unwrap();
                if k.desc {
                    o.reverse()
                } else {
                    o
                }
            }
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.cmp(&b)
}

fn run(engine: &mut Engine, budget: usize) {
    let mut guard = 0;
    while engine.has_work() {
        engine.step(budget);
        guard += 1;
        assert!(guard < 1_000_000, "engine does not finish");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]
    #[test]
    fn engine_matches_naive(
        (len, n_act, sched, cols) in schedules_strategy().prop_flat_map(|(len, n_act, s)| {
            let n = s.len();
            (Just(len), Just(n_act), Just(s), columns_strategy(n))
        }),
        filter in proptest::collection::vec(condition_strategy(), 0..4),
        sort in proptest::collection::vec(sortkey_strategy(), 0..4),
        sample in proptest::option::of((0u32..40, 0u32..4).prop_map(|(n, seed)| Sample { n, seed })),
        budget in prop_oneof![1usize..8, Just(1 << 20)],
    ) {
        let ds = dataset_with_columns(len, n_act, &sched, cols);
        let n = ds.n_persons();
        let mut want: Vec<u32> = (0..n as u32)
            .filter(|&p| filter.iter().all(|c| naive_test(c, naive_value(&ds, &c.col, p as usize))))
            .collect();
        let want_filtered = want.clone();
        if let Some(s) = sample.filter(|s| (s.n as usize) < want.len()) {
            want.sort_by_key(|&p| rank(p, s.seed));
            want.truncate(s.n as usize);
            want.sort_unstable();
        }
        want.sort_by(|&a, &b| naive_cmp(&ds, &sort, a, b));
        let want_profile = profile(&ds, &want);

        let mut engine = Engine::new(ds);
        let spec = ViewSpec { filter, sort, sample };
        engine.submit(7, spec.clone()).unwrap();
        run(&mut engine, budget);
        let v = engine.view(7).unwrap();
        prop_assert_eq!(&*v.rows, &want);
        prop_assert_eq!(&**v.profile.as_ref().unwrap(), &want_profile);
        prop_assert_eq!(&*v.filtered, &want_filtered);
        let mut m = want.clone();
        m.sort();
        prop_assert_eq!(v.members(), &m[..]);
        // Re-submitting the same spec is a no-op; a sort-only change reuses the filter.
        engine.submit(7, spec.clone()).unwrap();
        prop_assert!(!engine.has_work());
    }

    #[test]
    fn profile_matches_minute_grid((len, n_act, sched) in schedules_strategy()) {
        let ds = crate::testutil::dataset(len, n_act, &sched);
        let rows: Vec<u32> = (0..sched.len() as u32).collect();
        let got = profile(&ds, &rows);
        for t in 0..len {
            let mut want = vec![0u32; n_act];
            for p in 0..sched.len() {
                let c = sched[p].iter().rev().find(|s| s.0 as u32 <= t).unwrap().1;
                want[c as usize] += 1;
            }
            for c in 0..n_act {
                prop_assert_eq!(got[c * len as usize + t as usize], want[c]);
            }
        }
    }
}

#[test]
fn rows_arrive_before_profile_and_jobs_overtake_profiles() {
    let sched: Vec<Vec<(u16, u8)>> = (0..5000)
        .map(|i| vec![(0, 0), ((i % 50 + 1) as u16, 1)])
        .collect();
    let ds = dataset_with_columns(100, 2, &sched, vec![]);
    let mut engine = Engine::new(ds);
    engine.submit(1, ViewSpec::default()).unwrap();
    let mut saw_rows_without_profile = false;
    let mut submitted_second = false;
    while engine.has_work() {
        let r = engine.step(1000);
        if let Some(v) = engine.view(1) {
            if v.profile.is_none() {
                saw_rows_without_profile = true;
                if !submitted_second {
                    // A new job for another view overtakes view 1's profile job.
                    engine.submit(2, ViewSpec::default()).unwrap();
                    submitted_second = true;
                }
            }
        }
        if r.finished.contains(&2) {
            assert!(
                engine.view(1).unwrap().profile.is_none(),
                "view 2 rows should come first"
            );
        }
    }
    assert!(saw_rows_without_profile);
    assert!(engine.view(1).unwrap().profile.is_some());
    assert!(engine.view(2).unwrap().profile.is_some());
}

#[test]
fn newer_submit_cancels_older() {
    let sched: Vec<Vec<(u16, u8)>> = (0..1000)
        .map(|i| vec![(0, 0), ((i % 50 + 1) as u16, 1)])
        .collect();
    let ds = dataset_with_columns(100, 2, &sched, vec![]);
    let mut engine = Engine::new(ds);
    let by = |a| ViewSpec {
        filter: vec![],
        sort: vec![SortKey {
            col: ColRef::Virtual(Feature::minutes(a)),
            desc: false,
            order: None,
        }],
        sample: None,
    };
    engine.submit(1, by(0)).unwrap();
    engine.step(10);
    engine.submit(1, by(1)).unwrap();
    run(&mut engine, 100);
    assert!(engine.view(1).unwrap().profile.is_some());
    let v = engine.view(1).unwrap();
    assert_eq!(v.spec, by(1));
    assert_eq!(v.version, 0);
    // Minutes in activity 1 = 100 - (i % 50 + 1): largest for i % 50 == 0.
    assert_eq!(v.rows[0], 49);
}

#[test]
fn sample_change_reuses_filter_and_recomputes_profile() {
    let sched: Vec<Vec<(u16, u8)>> = (0..3000)
        .map(|i| vec![(0, 0), ((i % 50 + 1) as u16, 1)])
        .collect();
    let ds = dataset_with_columns(100, 2, &sched, vec![]);
    let mut engine = Engine::new(ds);
    let spec = |n: Option<u32>| ViewSpec {
        sample: n.map(|n| Sample { n, seed: 0 }),
        ..ViewSpec::default()
    };
    engine.submit(1, spec(None)).unwrap();
    run(&mut engine, 1 << 20);
    engine.submit(1, spec(Some(100))).unwrap();
    run(&mut engine, 1 << 20);
    let v = engine.view(1).unwrap();
    assert_eq!(v.filtered.len(), 3000);
    assert_eq!(v.rows.len(), 100);
    let total: u32 = v.profile.as_ref().unwrap()[..100].iter().sum::<u32>() / 100;
    assert!(total <= 100, "profile must count the sample only");
    // A sample at least as large as the filter keeps everyone.
    engine.submit(1, spec(Some(5000))).unwrap();
    run(&mut engine, 1 << 20);
    let v = engine.view(1).unwrap();
    assert!(v.sampled.is_none());
    assert_eq!(v.rows.len(), 3000);
}

#[test]
fn several_views_resort_together() {
    let sched: Vec<Vec<(u16, u8)>> = (0..3000)
        .map(|i| vec![(0, 0), (((i * 7919) % 50 + 1) as u16, 1)])
        .collect();
    let ds = dataset_with_columns(100, 2, &sched, vec![]);
    let mut engine = Engine::new(ds);
    for v in 1..=3 {
        engine.submit(v, ViewSpec::default()).unwrap();
    }
    run(&mut engine, 5000);
    let sorted = ViewSpec {
        sort: vec![SortKey {
            col: ColRef::Virtual(Feature::minutes(1)),
            desc: false,
            order: None,
        }],
        ..ViewSpec::default()
    };
    for v in 1..=3 {
        engine.submit(v, sorted.clone()).unwrap();
    }
    run(&mut engine, 5000);
    let want = engine.view(1).unwrap().rows.clone();
    for v in 1..=3 {
        assert_eq!(engine.view(v).unwrap().spec, sorted, "view {v}");
        assert_eq!(engine.view(v).unwrap().rows, want, "view {v}");
    }
}

#[test]
fn column_is_unusable_until_finished() {
    let sched: Vec<Vec<(u16, u8)>> = (0..10).map(|_| vec![(0, 0)]).collect();
    let cols = vec![("age", "numeric", None, ColumnData::I8(vec![5; 10]))];
    let mut ds = dataset_with_columns(100, 1, &sched, cols);
    ds.unload_column(0);
    // Allocated (as when the host starts copying data in) but not finished.
    ds.part_bytes_mut(crate::Part::Column(0)).unwrap();
    assert!(!ds.column_loaded(0));
    let mut engine = Engine::new(ds);
    let spec = ViewSpec {
        sort: vec![SortKey {
            col: ColRef::Attr { column: 0 },
            desc: false,
            order: None,
        }],
        ..ViewSpec::default()
    };
    assert!(engine.submit(1, spec.clone()).is_err());
    engine.ds.finish_column(0).unwrap();
    assert!(engine.ds.column_loaded(0));
    engine.submit(1, spec).unwrap();
}

#[test]
fn identical_resubmit_keeps_pending_profile() {
    let sched: Vec<Vec<(u16, u8)>> = (0..50).map(|p| vec![(0, 0), (1 + p % 40, 1)]).collect();
    let ds = dataset_with_columns(100, 2, &sched, vec![]);
    let mut engine = Engine::new(ds);
    let spec = ViewSpec::default();
    engine.submit(1, spec.clone()).unwrap();
    // Small steps: rows are installed before the profile is done.
    while engine.step(1).finished.is_empty() {}
    assert!(engine.view(1).unwrap().profile.is_none());
    engine.submit(1, spec).unwrap();
    run(&mut engine, 1);
    let rows = engine.view(1).unwrap().rows.clone();
    assert_eq!(
        engine.view(1).unwrap().profile,
        Some(std::sync::Arc::new(profile(&engine.ds, &rows)))
    );
}

#[test]
fn finished_parts_are_immutable_and_bad_filters_rejected() {
    let sched: Vec<Vec<(u16, u8)>> = (0..10).map(|_| vec![(0, 0)]).collect();
    let cols = vec![("age", "numeric", None, ColumnData::I8(vec![5; 10]))];
    let mut ds = dataset_with_columns(100, 1, &sched, cols);
    assert!(ds.part_bytes_mut(crate::Part::Segments).is_err());
    assert!(ds.part_bytes_mut(crate::Part::Offsets).is_err());
    assert!(ds.part_bytes_mut(crate::Part::Column(0)).is_err());
    assert!(ds.column_loaded(0));
    assert!(ds.finish_column(7).is_err());
    let mut engine = Engine::new(ds);
    let spec = ViewSpec {
        filter: vec![Condition {
            col: ColRef::Attr { column: 0 },
            test: Test::In {
                values: vec![u32::MAX],
                null: false,
            },
            negate: false,
        }],
        ..ViewSpec::default()
    };
    assert!(engine.submit(1, spec).is_err());
}

#[test]
fn unused_features_are_evicted_least_recently_used_first() {
    let sched: Vec<Vec<(u16, u8)>> = (0..50).map(|p| vec![(0, 0), (1 + p % 40, 1)]).collect();
    let ds = dataset_with_columns(100, 2, &sched, vec![]);
    let mut engine = Engine::new(ds);
    // Room for one unused i16 feature of 50 persons.
    engine.unused_feature_bytes = 100;
    let sort_by = |f: Feature| ViewSpec {
        sort: vec![SortKey {
            col: ColRef::Virtual(f),
            desc: false,
            order: None,
        }],
        ..ViewSpec::default()
    };
    let (a, b, c) = (
        Feature::minutes(0),
        Feature::minutes(1),
        Feature::first_start(1),
    );
    for f in [&a, &b, &c] {
        engine.submit(1, sort_by(f.clone())).unwrap();
        run(&mut engine, 1 << 20);
    }
    // `c` is in use, `b` is the most recent unused one, `a` is dropped.
    assert!(engine.has_feature(&c));
    assert!(engine.has_feature(&b));
    assert!(!engine.has_feature(&a));
    // A dropped feature is simply recomputed.
    engine.submit(1, sort_by(a.clone())).unwrap();
    run(&mut engine, 1 << 20);
    assert!(engine.has_feature(&a));
}
