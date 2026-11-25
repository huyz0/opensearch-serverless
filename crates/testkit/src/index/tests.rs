//! The vector generator and the files of a vector index.

use super::*;

fn unit(n: u64) -> f32 {
    n as f32 / (1u64 << 24) as f32
}

#[test]
fn three_dimensions_are_the_small_whole_numbers() {
    let spec = Spec::new("t", 10);
    assert_eq!(spec_vector(&spec, 3), Some(vec![3.0, 3.0, 0.0]));
    assert_eq!(spec_vector(&spec, 4), None);
}

#[test]
fn other_dimensions_are_drawn_from_the_document_number() {
    let spec = Spec::new("t", 10).with_vector_dimension(4);
    // The first four 24-bit draws of the generator seeded with `i`.
    let draws: [(usize, [u64; 4]); 2] = [
        (0, [0, 1_048_641, 10_165_892, 16_078_069]),
        (1, [14_424_951, 6_615_278, 8_062_926, 3_170_053]),
    ];
    for (i, expected) in draws {
        let got = spec_vector(&spec, i).expect("a vector");
        let want: Vec<f32> = expected.iter().map(|n| unit(*n)).collect();
        assert_eq!(got, want, "document {i}");
    }
    assert_eq!(spec_vector(&spec, 4), None, "every fifth has none");
    assert_eq!(spec_vector(&spec, 9), None);
    assert!(spec_vector(&spec, 5).is_some());
}

#[test]
fn a_vector_spec_writes_its_files_and_names_its_dimension() {
    let spec = Spec::new("v", 12)
        .segments_of(6)
        .with_vectors(VectorSimilarityFunction::Euclidean)
        .with_vector_dimension(8);
    assert!(mapping(&spec).contains(r#""dimension":8"#));
    let files = lucene_files(&spec);
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.iter().any(|n| n.starts_with("segments_")),
        "{names:?}"
    );
    assert_eq!(names.iter().filter(|n| n.ends_with(".si")).count(), 2);
    // Twelve documents, ten of them with eight floats each, are in the
    // two segments' compound files.
    let total: usize = files
        .iter()
        .filter(|(n, _)| n.ends_with(".cfs"))
        .map(|(_, b)| b.len())
        .sum();
    assert!(total >= 10 * 8 * 4, "{total} bytes");
}

#[test]
fn blobs_hold_the_descriptor_with_its_settings_the_manifest_and_every_file() {
    let spec = Spec::new("x", 4).with_settings(r#"{"a.b":"c"}"#);
    let out = blobs("pre/", &spec);
    // Segment ids are random, so the files are read back from the blobs.
    let shard = "pre/segments/x#x-uuid#0/";
    let files: Vec<(String, Vec<u8>)> = out[2..]
        .iter()
        .map(|(k, b)| {
            (
                k.strip_prefix(&format!("{shard}t=1/"))
                    .expect("file key")
                    .to_owned(),
                b.clone(),
            )
        })
        .collect();
    assert!(files.iter().any(|(n, _)| n.starts_with("segments_")));
    let text = |bytes: &[u8]| String::from_utf8_lossy(&bytes[8..]).into_owned();
    assert_eq!(out[0].0, "pre/indices/x");
    let descriptor = text(&out[0].1);
    assert!(descriptor.contains(r#""name":"x""#), "{descriptor}");
    assert!(
        descriptor.contains(r#""settings":{"a.b":"c"}"#),
        "{descriptor}"
    );
    assert_eq!(out[1].0, format!("{shard}manifest"));
    let manifest = text(&out[1].1);
    for (name, bytes) in &files {
        assert!(manifest.contains(&format!(r#""{name}":"t=1""#)), "{name}");
        assert!(manifest.contains(&format!(r#""{name}":{}"#, bytes.len())));
        let blob = out
            .iter()
            .find(|(k, _)| *k == format!("{shard}t=1/{name}"))
            .expect("file blob");
        assert_eq!(&blob.1, bytes);
        assert!(!bytes.is_empty());
    }
}
