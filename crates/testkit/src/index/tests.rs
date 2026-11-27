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

/// The documents of `spec`'s index as the `.si` files count them, and its
/// field names as the first `.fnm` file lists them.
fn segment(spec: Spec) -> (usize, String, usize) {
    let mut spec = spec;
    spec.compound = false;
    let files = lucene_files(&spec);
    let file = |ext: &str| {
        files
            .iter()
            .find(|(n, _)| n.ends_with(ext))
            .expect(ext)
            .1
            .clone()
    };
    // Every segment's `.si` holds its document count before three zero bytes
    // and the 0xff of its (absent) sort.
    let count: usize = files
        .iter()
        .filter(|(n, _)| n.ends_with(".si"))
        .map(|(_, si)| {
            (0..=u8::MAX)
                .find(|n| si.windows(8).any(|w| w == [0, 0, 0, *n, 0, 0, 0, 0xff]))
                .map_or(0, usize::from)
        })
        .sum();
    let fnm = file(".fnm");
    // Every field's doc-values generation is written as -1 (eight 0xff bytes).
    let no_gen = fnm.windows(8).filter(|w| *w == [0xff; 8]).count();
    (count, String::from_utf8_lossy(&fnm).into_owned(), no_gen)
}

/// Whether any file of `spec`'s index (uncompounded) ending in `ext` holds `needle`.
fn file_holds(mut spec: Spec, ext: &str, needle: &[u8]) -> bool {
    spec.compound = false;
    let files = lucene_files(&spec);
    assert!(files.iter().any(|(n, _)| n.ends_with(ext)), "no {ext} file");
    files
        .iter()
        .filter(|(n, _)| n.ends_with(ext))
        .any(|(_, b)| b.windows(needle.len()).any(|w| w == needle))
}

#[test]
fn a_nested_index_is_a_block_a_document_and_a_deep_one_has_grandchildren() {
    let (flat, flat_fields, flat_gens) = segment(Spec::new("s", 6));
    assert_eq!(flat, 6);
    assert!(!flat_fields.contains("_nested_path") && !flat_fields.contains("c.w"));
    let (nested, nested_fields, nested_gens) = segment(Spec::new("s", 6).with_nested());
    assert_eq!(nested, 6 * (1 + NESTED_CHILDREN));
    assert!(nested_fields.contains("_nested_path") && nested_fields.contains("c.w"));
    assert!(!nested_fields.contains("c.d.v"));
    let (deep, deep_fields, deep_gens) = segment(Spec::new("s", 6).with_nested_deep());
    assert_eq!(deep, 6 * (1 + 2 * NESTED_CHILDREN));
    assert!(deep_fields.contains("c.w") && deep_fields.contains("c.d.v"));
    // The fields the nested levels add (c.k among them) add doc-values generations.
    assert_eq!(nested_gens, flat_gens + 4);
    assert_eq!(deep_gens, nested_gens + 1);
    // A grandchild's word is indexed (the terms file) and has a doc value
    // (the doc-values data); a child's is too.
    for ext in [".tim", ".dvd"] {
        let has = |spec: Spec, word: &[u8]| file_holds(spec, ext, word);
        assert!(has(Spec::new("s", 6).with_nested_deep(), b"v0-0"), "{ext}");
        assert!(!has(Spec::new("s", 6).with_nested(), b"v0-0"), "{ext}");
        assert!(has(Spec::new("s", 6).with_nested(), b"w0-0"), "{ext}");
    }
}

#[test]
fn the_mapping_and_the_source_name_the_levels_that_exist() {
    let spec = |depth: u8| Spec {
        nested: depth,
        ..Spec::new("s", 2)
    };
    assert!(!mapping(&spec(0)).contains(r#""c""#));
    assert!(!doc_source(&spec(0), 0).contains(r#""c":"#));
    let one = mapping(&spec(1));
    assert!(
        one.contains(r#""c":{"type":"nested""#) && !one.contains(r#""d""#),
        "{one}"
    );
    assert!(doc_source(&spec(1), 0).contains(r#""c":[{"w":"w0-0"},{"w":"w0-1"}]"#));
    let two = mapping(&spec(2));
    assert!(
        two.contains(r#""d":{"type":"nested","properties":{"v":{"type":"keyword"}}}"#),
        "{two}"
    );
    let source = doc_source(&spec(2), 1);
    assert!(
        source.contains(r#""c":[{"w":"w1-0","d":[{"v":"v1-0"}]},{"w":"w1-1","d":[{"v":"v1-1"}]}]"#),
        "{source}"
    );
}

#[test]
fn a_nested_child_number_is_ten_times_its_document_plus_its_position() {
    for (i, j, want) in [
        (0, 0, 0),
        (0, 1, 1),
        (1, 0, 10),
        (1, 1, 11),
        (7, 1, 71),
        (9_999, 1, 99_991),
    ] {
        assert_eq!(nested_number(i, j), want, "document {i}, child {j}");
    }
}
