//! The checkpoint: round trips and refusals.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::*;

#[test]
fn a_restored_model_scores_the_next_thousand_points_as_the_one_that_kept_running() {
    let mut n = Lcg(2);
    let series: Vec<Option<f32>> = (0..1_600).map(|t| Some(seasonal(t, &mut n))).collect();
    let mut ws = Workspace::new(&small(1, 3));
    let mut live = Model::new(small(1, 3)).unwrap();
    run(&mut live, &mut ws, &series[..600]);
    let bytes = live.checkpoint();
    let mut restored = Model::restore(&bytes).expect("restores");
    assert_eq!(restored, live);
    assert_eq!(restored.checkpoint(), bytes);
    for (t, v) in series.iter().enumerate().skip(600) {
        let w = [v.unwrap()];
        let a = live.feed(&mut ws, t as u64, Some(&w)).unwrap();
        let b = restored.feed(&mut ws, t as u64, Some(&w)).unwrap();
        assert_eq!(a, b, "diverged at {t}");
    }
    assert_eq!(live.checkpoint(), restored.checkpoint());
}

#[test]
fn a_checkpoint_that_is_wrong_in_any_way_is_an_error_never_a_panic() {
    let p = small(1, 5);
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    let mut n = Lcg(3);
    let series: Vec<Option<f32>> = (0..700).map(|t| Some(seasonal(t, &mut n))).collect();
    run(&mut m, &mut ws, &series);
    let good = m.checkpoint();
    assert!(Model::restore(&[]).is_err());
    assert!(Model::restore(&good[..good.len() / 2]).is_err());
    let mut version = good.clone();
    version[4] = 99;
    assert_eq!(
        Model::restore(&version),
        Err(crate::RestoreError::Version(99))
    );
    // a flipped bit anywhere is caught by the checksum
    for at in (0..good.len()).step_by(997) {
        let mut b = good.clone();
        b[at] ^= 0x10;
        assert!(Model::restore(&b).is_err(), "bit flip at {at}");
    }
    // structural damage with a valid checksum: must be refused or survivable, never a panic
    let sum = |b: &mut Vec<u8>| {
        let n = b.len() - 8;
        let h = {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for &x in &b[..n] {
                h ^= u64::from(x);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            h
        };
        b[n..].copy_from_slice(&h.to_le_bytes());
    };
    let mut refused = 0;
    for at in (40..good.len() - 8).step_by(307) {
        for flip in [0xFFu8, 0x01, 0x80] {
            let mut b = good.clone();
            b[at] ^= flip;
            sum(&mut b);
            match Model::restore(&b) {
                Err(_) => refused += 1,
                Ok(mut restored) => {
                    let mut ws2 = Workspace::new(restored.params());
                    for t in 700..720u64 {
                        let _ = restored.feed(&mut ws2, t, Some(&[50.0]));
                    }
                }
            }
        }
    }
    assert!(
        refused > 20,
        "structural damage was mostly accepted: {refused}"
    );
}

#[test]
fn errors_read_as_sentences_and_fixed_imputation_survives_a_checkpoint() {
    let mut p = small(2, 8);
    p.imputation = Imputation::Fixed(vec![1.5, -2.0]);
    let mut ws = Workspace::new(&p);
    let mut m = Model::new(p).unwrap();
    assert!(!m.ready());
    assert_eq!(m.last_fed_slot(), None);
    m.feed(&mut ws, 3, Some(&[1.0, 2.0])).unwrap();
    assert_eq!(m.last_fed_slot(), Some(3));
    let back = Model::restore(&m.checkpoint()).unwrap();
    assert_eq!(back.params().imputation, Imputation::Fixed(vec![1.5, -2.0]));
    for e in [
        FeedError::Width,
        FeedError::NotFinite,
        FeedError::Stale,
        FeedError::Workspace,
    ] {
        assert!(!e.to_string().is_empty());
    }
    for e in [
        crate::RestoreError::NotACheckpoint,
        crate::RestoreError::Version(2),
        crate::RestoreError::Corrupt,
        crate::RestoreError::Params("x".into()),
        crate::RestoreError::Inconsistent("y"),
    ] {
        assert!(!e.to_string().is_empty());
    }
    assert!(Params::new(1, 0).validate().is_ok());
    assert_eq!(
        Params::new(0, 0).validate().unwrap_err().to_string(),
        "features is out of range"
    );
}

#[test]
fn a_free_node_that_holds_a_leaf_is_refused_not_a_panic() {
    let (mut m, _) = alternating(30, 400);
    let tree = &mut m.forest.members[0].tree;
    let free = *tree.free.first().expect("a free node");
    tree.nodes[free as usize] = crate::tree::Node {
        mass: 1,
        point: u32::MAX,
        ..crate::tree::Node::EMPTY
    };
    assert!(Model::restore(&m.checkpoint()).is_err());
}

#[test]
fn a_leaf_whose_mass_disagrees_with_the_points_held_under_it_is_refused() {
    let (mut m, _) = alternating(31, 400);
    let tree = &mut m.forest.members[0].tree;
    let leaves: Vec<usize> = (0..tree.nodes.len())
        .filter(|&i| tree.nodes[i].is_leaf() && tree.nodes[i].mass > 1)
        .collect();
    assert!(leaves.len() >= 2, "two leaves of duplicates");
    // moving one unit of mass between leaves keeps the total, so only a per-leaf check sees it
    tree.nodes[leaves[0]].mass += 1;
    tree.nodes[leaves[1]].mass -= 1;
    assert!(Model::restore(&m.checkpoint()).is_err());
}
