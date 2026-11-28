//! Detection quality on synthetic series.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::*;

#[test]
fn a_flat_series_stays_at_grade_zero_and_duplicates_do_not_break_the_trees() {
    let p = small(1, 4);
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    let series = vec![Some(7.0f32); 2_000];
    let out = run(&mut m, &mut ws, &series);
    assert!(out.iter().flatten().all(|s| s.grade == 0.0));
    assert!(
        m.forest
            .members
            .iter()
            .all(|mem| mem.tree.verify(m.forest.len(), 4).is_some())
    );
}

fn rates(
    out: &[Option<Scored>],
    anomalies: &[usize],
    shingle: usize,
    from: usize,
) -> (usize, usize) {
    let near = |t: usize| anomalies.iter().any(|&a| t >= a && t < a + shingle);
    let mut steady = 0;
    let mut flagged = 0;
    for (t, s) in out.iter().enumerate().skip(from) {
        if let Some(s) = s
            && !near(t)
        {
            steady += 1;
            flagged += usize::from(s.grade > 0.0);
        }
    }
    (steady, flagged)
}

#[test]
fn every_injected_spike_is_flagged_within_two_windows_and_steady_windows_rarely() {
    let mut n = Lcg(11);
    let spikes: Vec<usize> = (0..10).map(|k| 1_200 + k * 170).collect();
    let series: Vec<Option<f32>> = (0..3_000u64)
        .map(|t| {
            let v = seasonal(t, &mut n);
            Some(if spikes.contains(&(t as usize)) {
                v + 120.0
            } else {
                v
            })
        })
        .collect();
    let p = small(1, 21);
    let shingle = p.shingle_size;
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    let out = run(&mut m, &mut ws, &series);
    for &s in &spikes {
        let hit = (s..s + 3).any(|t| out[t].is_some_and(|x| x.grade > 0.0));
        assert!(hit, "spike at {s} not flagged: {:?}", &out[s..s + 3]);
    }
    let (steady, flagged) = rates(&out, &spikes, shingle, 0);
    assert!(steady > 2_000);
    assert!(
        flagged * 100 <= steady,
        "{flagged} of {steady} steady windows flagged"
    );
}

#[test]
fn a_level_shift_is_flagged_and_the_grade_returns_to_zero_within_a_sample() {
    let mut n = Lcg(12);
    let shift = 1_500u64;
    let series: Vec<Option<f32>> = (0..3_000u64)
        .map(|t| Some(seasonal(t, &mut n) + if t >= shift { 60.0 } else { 0.0 }))
        .collect();
    let p = small(1, 22);
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    let out = run(&mut m, &mut ws, &series);
    let hit = (shift as usize..shift as usize + 3).any(|t| out[t].is_some_and(|x| x.grade > 0.0));
    assert!(hit, "the shift was not flagged: {:?}", &out[1_500..1_503]);
    let later = &out[shift as usize + 256..];
    let flagged = later.iter().flatten().filter(|s| s.grade > 0.0).count();
    assert!(
        flagged * 100 <= later.len(),
        "{flagged} of {} flagged after recovery",
        later.len()
    );
}

#[test]
fn a_stretch_of_missing_windows_does_not_flood_the_grades() {
    let mut n = Lcg(13);
    let series: Vec<Option<f32>> = (0..3_000u64)
        .map(|t| {
            if (2_000..2_005).contains(&t) {
                None
            } else {
                Some(seasonal(t, &mut n))
            }
        })
        .collect();
    let p = small(1, 23);
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    let out = run(&mut m, &mut ws, &series);
    let (steady, flagged) = rates(&out, &[], 4, 600);
    assert!(flagged * 100 <= steady, "{flagged} of {steady} flagged");
    assert!(
        out[2_000..2_005].iter().all(Option::is_none),
        "no score for a window without data"
    );
}
