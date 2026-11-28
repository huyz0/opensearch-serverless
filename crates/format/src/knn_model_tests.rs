#![cfg(test)]

use super::*;

fn flat() -> (ModelHeader, Vec<f32>) {
    let header = ModelHeader {
        space: ModelSpace::L2,
        dimension: 3,
        nlist: 2,
        encoder: ModelEncoder::Flat,
    };
    (header, vec![1.0, 2.0, 3.0, -1.0, 0.5, 0.0])
}

#[test]
fn a_flat_model_reads_back_its_centroids() {
    let (header, centroids) = flat();
    let blob = encode(&header, &centroids, &[]).unwrap();
    assert_eq!(Some(blob.len()), header.encoded_len());
    let view = parse(&blob).unwrap();
    assert_eq!(*view.header(), header);
    assert_eq!(
        view.centroid(1).unwrap().collect::<Vec<_>>(),
        [-1.0, 0.5, 0.0]
    );
    assert!(view.centroid(2).is_none());
    assert_eq!(view.codebook_floats().count(), 0);
}

#[test]
fn a_pq_model_carries_its_codebooks() {
    let header = ModelHeader {
        space: ModelSpace::InnerProduct,
        dimension: 4,
        nlist: 1,
        encoder: ModelEncoder::Pq { m: 2 },
    };
    let books: Vec<f32> = (0..2 * PQ_ENTRIES * 2).map(|i| i as f32).collect();
    let blob = encode(&header, &[0.0; 4], &books).unwrap();
    let view = parse(&blob).unwrap();
    assert_eq!(view.codebook_floats().collect::<Vec<_>>(), books);
    assert_eq!(*view.header(), header);
}

#[test]
fn a_flipped_byte_a_truncation_and_trailing_bytes_are_refused() {
    let (header, centroids) = flat();
    let blob = encode(&header, &centroids, &[]).unwrap();
    let mut flipped = blob.clone();
    flipped[HEADER] ^= 1;
    assert!(matches!(parse(&flipped), Err(FormatError::Corrupt { .. })));
    assert!(matches!(
        parse(&blob[..blob.len() - 1]),
        Err(FormatError::Truncated { .. })
    ));
    assert!(matches!(
        parse(&blob[..10]),
        Err(FormatError::Truncated { .. })
    ));
    let mut longer = blob.clone();
    longer.push(0);
    assert!(matches!(parse(&longer), Err(FormatError::Corrupt { .. })));
    let mut other = blob;
    other[4] = 2;
    assert!(matches!(
        parse(&other),
        Err(FormatError::Unsupported { version: 2, .. })
    ));
}

#[test]
fn impossible_shapes_are_refused_before_anything_is_allocated() {
    let pq = |dimension, m| ModelHeader {
        space: ModelSpace::L2,
        dimension,
        nlist: 1,
        encoder: ModelEncoder::Pq { m },
    };
    assert!(pq(5, 2).encoded_len().is_none());
    assert!(pq(4, 0).encoded_len().is_none());
    let huge = ModelHeader {
        space: ModelSpace::L2,
        dimension: u32::MAX,
        nlist: u32::MAX,
        encoder: ModelEncoder::Flat,
    };
    assert!(huge.encoded_len().is_none());
    assert!(encode(&huge, &[], &[]).is_err());
    let (header, _) = flat();
    assert!(encode(&header, &[1.0], &[]).is_err());
}
