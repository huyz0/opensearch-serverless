//! Unit tests: determinism, bounds, shingling, imputation, the checkpoint
//! and detection quality on synthetic series.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

pub(super) use crate::{FeedError, Imputation, Model, Params, Scored, Workspace};

mod checkpoint;
mod quality;

/// A small deterministic noise source (not the model's).
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

fn seasonal(t: u64, noise: &mut Lcg) -> f32 {
    50.0 + 10.0 * (t as f32 * std::f32::consts::TAU / 60.0).sin() + 2.0 * (noise.unit() - 0.5)
}

fn alternating(seed: u64, steps: u64) -> (Model, Workspace) {
    let mut p = small(1, seed);
    p.sample_size = 32;
    p.min_train_samples = 16;
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    for t in 0..steps {
        m.feed(&mut ws, t, Some(&[if t % 2 == 0 { 1.0 } else { 9.0 }]))
            .unwrap();
    }
    (m, ws)
}

fn small(features: usize, seed: u64) -> Params {
    let mut p = Params::new(features, seed);
    p.shingle_size = 4;
    p
}

fn run(model: &mut Model, ws: &mut Workspace, series: &[Option<f32>]) -> Vec<Option<Scored>> {
    series
        .iter()
        .enumerate()
        .map(|(t, v)| {
            let w = v.map(|x| [x]);
            model
                .feed(ws, t as u64, w.as_ref().map(|a| a.as_slice()))
                .expect("feed")
        })
        .collect()
}

#[test]
fn the_same_seed_and_input_give_the_same_scores_and_checkpoint() {
    let series: Vec<Option<f32>> = {
        let mut n = Lcg(1);
        (0..600).map(|t| Some(seasonal(t, &mut n))).collect()
    };
    let (mut a, mut b) = (
        Model::new(small(1, 9)).unwrap(),
        Model::new(small(1, 9)).unwrap(),
    );
    let mut ws = Workspace::new(a.params());
    let ra = run(&mut a, &mut ws, &series);
    let rb = run(&mut b, &mut ws, &series);
    assert_eq!(ra, rb);
    assert_eq!(a.checkpoint(), b.checkpoint());
    let mut c = Model::new(small(1, 10)).unwrap();
    assert_ne!(
        run(&mut c, &mut ws, &series),
        ra,
        "another seed gives another forest"
    );
}

#[test]
fn parameters_out_of_range_are_refused_by_name() {
    type Change = Box<dyn Fn(&mut Params)>;
    let cases: Vec<(&str, Change)> = vec![
        ("number_of_trees", Box::new(|p| p.trees = 0)),
        ("number_of_trees", Box::new(|p| p.trees = 101)),
        ("sample_size", Box::new(|p| p.sample_size = 7)),
        ("sample_size", Box::new(|p| p.sample_size = 2_049)),
        ("shingle_size", Box::new(|p| p.shingle_size = 0)),
        ("shingle_size", Box::new(|p| p.shingle_size = 61)),
        ("features", Box::new(|p| p.features = 0)),
        (
            "features times shingle_size",
            Box::new(|p| (p.features, p.shingle_size) = (64, 9)),
        ),
        ("time_decay", Box::new(|p| p.time_decay = -0.1)),
        ("time_decay", Box::new(|p| p.time_decay = f64::NAN)),
        ("min_train_samples", Box::new(|p| p.min_train_samples = 1)),
        (
            "fixed imputation values",
            Box::new(|p| p.imputation = Imputation::Fixed(vec![1.0, 2.0])),
        ),
    ];
    for (name, change) in cases {
        let mut p = Params::new(1, 0);
        change(&mut p);
        let err = Model::new(p).expect_err(name);
        assert!(err.0.contains(name), "{name}: {err}");
    }
    assert!(Model::new(Params::new(1, 0)).is_ok());
}

/// The bytes a built model holds: the capacity of every array it owns.
fn held_bytes(m: &Model) -> usize {
    use std::mem::size_of;
    let f = &m.forest;
    let mut total = f.pts.data.capacity() * 4 + m.window_bytes();
    for mem in &f.members {
        total += mem.tree.nodes.capacity() * size_of::<crate::tree::Node>()
            + mem.tree.free.capacity() * 4
            + mem.held.capacity() * size_of::<crate::forest::Held>()
            + mem.slot_of_point.capacity() * 4;
    }
    total + m.shingle.windows.capacity() * 4 + m.shingle.last.capacity() * 4
}

impl Model {
    fn window_bytes(&self) -> usize {
        (self.params.features + self.params.dims()) * 4
    }
}

#[test]
fn what_a_model_reserves_covers_what_it_holds_across_a_grid() {
    for &(trees, sample, shingle, features) in &[
        (1, 8, 1, 1),
        (30, 256, 8, 5),
        (30, 256, 60, 5),
        (100, 2_048, 8, 1),
        (10, 64, 30, 16),
    ] {
        let mut p = Params::new(features, 1);
        (p.trees, p.sample_size, p.shingle_size) = (trees, sample, shingle);
        p.min_train_samples = 8.min(sample);
        let mut model = Model::new(p.clone()).unwrap();
        // fill it well past its ring so every array is at its working size
        let mut ws = Workspace::new(&p);
        let mut n = Lcg(5);
        for t in 0..(p.store_capacity() as u64 + 50).min(1_500) {
            let w: Vec<f32> = (0..features).map(|_| n.unit() * 100.0).collect();
            model.feed(&mut ws, t, Some(&w)).unwrap();
        }
        assert!(
            p.reserved_bytes() >= held_bytes(&model),
            "{trees}x{sample} shingle {shingle} features {features}: reserved {} < held {}",
            p.reserved_bytes(),
            held_bytes(&model)
        );
        assert!(ws.fits(&p));
    }
}

#[test]
fn a_point_is_the_last_windows_oldest_first() {
    let mut p = Params::new(2, 1);
    p.shingle_size = 3;
    let m = Model::new(p).unwrap();
    let mut s = m.shingle.clone();
    for k in 1..=5 {
        s.push(&[k as f32, 10.0 * k as f32]);
    }
    let mut out = vec![0.0; 6];
    s.point(&mut out);
    assert_eq!(out, vec![3.0, 30.0, 4.0, 40.0, 5.0, 50.0]);
}

fn imputed_point(how: Imputation, gap_windows: u64) -> Vec<f32> {
    let mut p = Params::new(2, 1);
    p.shingle_size = 3;
    p.max_gap = 3;
    p.imputation = how;
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    m.feed(&mut ws, 0, Some(&[1.0, 2.0])).unwrap();
    m.feed(&mut ws, 1, Some(&[3.0, 4.0])).unwrap();
    m.feed(&mut ws, 2 + gap_windows, Some(&[5.0, 6.0])).unwrap();
    let mut out = vec![0.0; 6];
    m.shingle.point(&mut out);
    out
}

#[test]
fn a_missing_window_is_filled_in_as_each_option_says() {
    assert_eq!(
        imputed_point(Imputation::Zero, 1),
        vec![3.0, 4.0, 0.0, 0.0, 5.0, 6.0]
    );
    assert_eq!(
        imputed_point(Imputation::Fixed(vec![9.0, 8.0]), 1),
        vec![3.0, 4.0, 9.0, 8.0, 5.0, 6.0]
    );
    assert_eq!(
        imputed_point(Imputation::Previous, 1),
        vec![3.0, 4.0, 3.0, 4.0, 5.0, 6.0]
    );
    assert_eq!(
        imputed_point(Imputation::Previous, 2),
        vec![3.0, 4.0, 3.0, 4.0, 5.0, 6.0]
    );
}

#[test]
fn a_gap_past_max_gap_restarts_the_shingle() {
    let mut p = Params::new(1, 1);
    p.shingle_size = 3;
    p.max_gap = 2;
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    for t in 0..5 {
        m.feed(&mut ws, t, Some(&[t as f32])).unwrap();
    }
    assert!(m.shingle.full());
    m.feed(&mut ws, 8, Some(&[8.0])).unwrap(); // 3 slots missing: past max_gap
    assert_eq!(
        m.shingle.filled, 1,
        "the shingle restarted at the new window"
    );
    m.feed(&mut ws, 10, Some(&[10.0])).unwrap(); // 1 missing: imputed
    assert_eq!(m.shingle.filled, 3);
}

#[test]
fn bad_windows_and_stale_slots_are_refused() {
    let p = Params::new(2, 1);
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p.clone()).unwrap();
    assert_eq!(m.feed(&mut ws, 0, Some(&[1.0])), Err(FeedError::Width));
    assert_eq!(
        m.feed(&mut ws, 0, Some(&[1.0, f32::NAN])),
        Err(FeedError::NotFinite)
    );
    m.feed(&mut ws, 5, Some(&[1.0, 2.0])).unwrap();
    assert_eq!(m.feed(&mut ws, 5, Some(&[1.0, 2.0])), Err(FeedError::Stale));
    assert_eq!(m.feed(&mut ws, 4, None), Err(FeedError::Stale));
    let tiny = Workspace::new(&Params::new(1, 1));
    let mut tiny = tiny;
    assert_eq!(m.feed(&mut tiny, 6, None), Err(FeedError::Workspace));
}

/// The steady windows are those with no injected anomaly in the last shingle.
#[test]
fn a_gap_limit_past_its_bound_is_refused() {
    let mut p = Params::new(1, 1);
    p.max_gap = crate::params::MAX_GAP + 1;
    assert!(Model::new(p).unwrap_err().0.contains("max_gap"));
    let mut p = Params::new(1, 1);
    p.max_gap = crate::params::MAX_GAP;
    assert!(Model::new(p).is_ok());
}

#[test]
fn extreme_values_are_refused_and_the_thresholder_stays_finite() {
    let (mut m, mut ws) = alternating(32, 300);
    for v in [f32::MAX, -f32::MAX, 1.0e30, -1.0e19] {
        assert_eq!(
            m.feed(&mut ws, 1_000, Some(&[v])),
            Err(FeedError::OutOfRange),
            "{v}"
        );
    }
    // the largest accepted magnitudes score finitely and the model still round-trips
    for (t, v) in [(1_000u64, 1.0e18f32), (1_001, -1.0e18), (1_002, 1.0e18)] {
        let s = m.feed(&mut ws, t, Some(&[v])).unwrap();
        assert!(s.is_none_or(|s| s.score.is_finite() && s.grade.is_finite()));
    }
    assert!(Model::restore(&m.checkpoint()).is_ok());
}
