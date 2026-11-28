//! The trainer's tests; compiled only under `#[cfg(test)]`.

use oss_format::{ModelEncoder, ModelSpace, PQ_ENTRIES, parse_model};

use super::*;

/// `per` points around each of `centres`, spread by a fixed pseudo-random step.
fn blobs(centres: &[[f32; 4]], per: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let mut state = 7u32;
    for c in centres {
        for _ in 0..per {
            for x in c {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                out.push(x + ((state >> 16) as f32 / 65_536.0 - 0.5) * 0.1);
            }
        }
    }
    out
}

fn spec(nlist: usize, pq_m: Option<usize>) -> TrainSpec {
    TrainSpec {
        dimension: 4,
        nlist,
        space: ModelSpace::L2,
        pq_m,
        points_per_centroid: 256,
    }
}

const CENTRES: [[f32; 4]; 3] = [[0.0; 4], [10.0; 4], [-10.0, 10.0, -10.0, 10.0]];

#[test]
fn a_flat_model_finds_the_clusters() {
    let sample = blobs(&CENTRES, 50);
    let blob = train(&sample, &spec(3, None)).unwrap();
    let view = parse_model(&blob).unwrap();
    assert_eq!(view.header().nlist, 3);
    assert_eq!(view.header().encoder, ModelEncoder::Flat);
    for c in CENTRES {
        let near = (0..3)
            .map(|i| {
                let got: Vec<f32> = view.centroid(i).unwrap().collect();
                got.iter().zip(c).map(|(a, b)| (a - b).abs()).sum::<f32>()
            })
            .fold(f32::MAX, f32::min);
        assert!(near < 0.5, "no centroid near {c:?}: {near}");
    }
    assert_eq!(view.codebook_floats().count(), 0);
}

#[test]
fn a_pq_model_carries_codebooks_trained_on_residuals() {
    let sample = blobs(&CENTRES, 200);
    let blob = train(&sample, &spec(3, Some(2))).unwrap();
    let view = parse_model(&blob).unwrap();
    assert_eq!(view.header().encoder, ModelEncoder::Pq { m: 2 });
    assert_eq!(view.codebook_floats().count(), 2 * PQ_ENTRIES * 2);
    // Residuals sit near zero, so the codebook entries do too.
    assert!(view.codebook_floats().all(|v| v.abs() < 1.0));
}

#[test]
fn too_few_vectors_fail_with_the_recorded_message_shape() {
    let sample = blobs(&CENTRES, 1);
    let failure = train(&sample, &spec(4, None)).unwrap_err();
    assert_eq!(
        failure.0,
        "Number of training points (3) should be at least as large as number of clusters (4)"
    );
    let few = blobs(&CENTRES, 10);
    let failure = train(&few, &spec(3, Some(2))).unwrap_err();
    assert_eq!(
        failure.0,
        "Number of training points (30) should be at least as large as number of clusters (256)"
    );
}

#[test]
fn a_shape_that_cannot_train_is_refused() {
    let sample = blobs(&CENTRES, 50);
    assert!(train(&sample[..sample.len() - 1], &spec(3, None)).is_err());
    assert!(train(&sample, &spec(3, Some(3))).is_err());
    assert!(train(&sample, &spec(0, None)).is_err());
}

#[test]
fn the_same_sample_trains_the_same_model() {
    let sample = blobs(&CENTRES, 50);
    assert_eq!(
        train(&sample, &spec(3, None)).unwrap(),
        train(&sample, &spec(3, None)).unwrap()
    );
}

#[test]
fn inner_product_centroids_are_unit_length() {
    let sample = blobs(&CENTRES[1..], 50);
    let mut s = spec(2, None);
    s.space = ModelSpace::InnerProduct;
    let blob = train(&sample, &s).unwrap();
    let view = parse_model(&blob).unwrap();
    for i in 0..2 {
        let norm: f32 = view.centroid(i).unwrap().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "norm {norm}");
    }
}
