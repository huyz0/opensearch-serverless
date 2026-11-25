//! docs/specs/stock-snapshot-restore AC-2 and AC-5: the blobs the server's
//! own `INDEX_METADATA_FORMAT` and `INDEX_SHARD_SNAPSHOT_FORMAT` write
//! (`fixtures/snapshots/stock-metadata/GenMetadata.java`) decode, plain and
//! deflated, and a shard snapshot reads to the files it lists, their chunks
//! and inline copies; a hostile one is refused.

use oss_format::{
    FormatError, STOCK_MAX_CHUNKS, STOCK_MAX_FILES, StockEnvelopeLimits, StockShardSnapshot,
    stock_blob_to_json,
};

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../fixtures/snapshots/stock-metadata/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn json(name: &str, codec: &str) -> Vec<u8> {
    let mut out = Vec::new();
    stock_blob_to_json(
        &fixture(name),
        codec,
        &StockEnvelopeLimits::default(),
        &mut out,
    )
    .expect("decodes");
    out
}

fn parse(text: &str) -> Result<StockShardSnapshot, FormatError> {
    StockShardSnapshot::parse(text.as_bytes())
}

fn one(file: &str) -> Result<StockShardSnapshot, FormatError> {
    parse(&format!(r#"{{"name":"s","files":[{file}]}}"#))
}

#[test]
fn the_servers_index_metadata_decodes_plain_and_deflated_alike() {
    let plain = json("index-metadata.plain.dat", "index-metadata");
    assert_eq!(plain, json("index-metadata.deflate.dat", "index-metadata"));
    let text = String::from_utf8(plain).expect("UTF-8");
    assert!(text.starts_with(r#"{"logs":{"version":1,"#), "{text}");
    assert!(text.contains(r#""index.number_of_shards":"2""#), "{text}");
    assert!(text.contains(r#""aliases":{"logs-alias":{}}"#), "{text}");
}

#[test]
fn the_servers_shard_snapshot_reads_to_its_files() {
    let plain = json("snapshot.plain.dat", "snapshot");
    assert_eq!(plain, json("snapshot.deflate.dat", "snapshot"));
    let shard = StockShardSnapshot::parse(&plain).expect("parses");
    assert_eq!(shard.name, "snap-1");
    assert_eq!(shard.files.len(), 2);

    let big = &shard.files[0];
    assert_eq!(big.name, "__bigBlobUuid1");
    assert_eq!(big.physical_name, "_0.cfs");
    assert_eq!(big.length, 12_000_000);
    assert_eq!(big.checksum, "1xsyk1b");
    assert_eq!(big.part_size, Some(5_000_000));
    assert_eq!(big.written_by.as_deref(), Some("10.5.1"));
    assert_eq!(big.inline.as_deref(), Some(&[1u8, 2, 3][..]));
    assert!(!big.is_virtual());
    assert_eq!(big.parts(), 3);
    assert_eq!(big.part_name(0), "__bigBlobUuid1.part0");
    assert_eq!(big.part_name(2), "__bigBlobUuid1.part2");

    let small = &shard.files[1];
    assert_eq!(small.physical_name, "segments_3");
    assert_eq!(small.part_size, None);
    assert_eq!(small.inline, None);
    assert_eq!(small.parts(), 1);
    assert_eq!(small.part_name(0), "__smallBlobUuid2");
}

#[test]
fn the_parts_of_a_file_are_its_length_over_its_part_size_rounded_up() {
    let parts = |length: u64, part: u64| {
        let f = one(&format!(
            r#"{{"name":"__a","physical_name":"_0.cfs","length":{length},"checksum":"c","part_size":{part}}}"#
        ))
        .expect("parses");
        f.files[0].parts()
    };
    assert_eq!(parts(10, 5), 2);
    assert_eq!(parts(11, 5), 3);
    assert_eq!(parts(9, 5), 2);
    assert_eq!(parts(1, 5), 1);
    assert_eq!(parts(5, 5), 1);
    assert_eq!(parts(0, 5), 1);
    // A part size of zero means the file is not split, and is not kept.
    assert_eq!(parts(1_000, 0), 1);
    let zero =
        one(r#"{"name":"__a","physical_name":"_0.cfs","length":9,"checksum":"c","part_size":0}"#)
            .expect("parses");
    assert_eq!(zero.files[0].part_size, None);
}

#[test]
fn a_file_may_have_exactly_the_most_chunks_and_no_more() {
    let at = |n: u64| {
        one(&format!(
            r#"{{"name":"__a","physical_name":"_0.cfs","length":{n},"checksum":"c","part_size":1}}"#
        ))
    };
    assert!(at(STOCK_MAX_CHUNKS).is_ok());
    assert!(matches!(
        at(STOCK_MAX_CHUNKS + 1),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_shard_may_list_exactly_the_most_files_and_no_more() {
    let file = r#"{"name":"__a","physical_name":"_0.cfs","length":1,"checksum":"c"}"#;
    let with = |n: usize| {
        let files = vec![file; n].join(",");
        parse(&format!(r#"{{"name":"s","files":[{files}]}}"#))
    };
    assert_eq!(
        with(STOCK_MAX_FILES).expect("at the limit").files.len(),
        STOCK_MAX_FILES
    );
    assert!(matches!(
        with(STOCK_MAX_FILES + 1),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_virtual_file_is_its_inline_copy_and_nothing_else() {
    let ok = one(r#"{"name":"v__x","physical_name":"segments_1","length":3,"checksum":"c","meta_hash":"AQID"}"#)
        .expect("parses");
    assert!(ok.files[0].is_virtual());
    assert_eq!(ok.files[0].inline.as_deref(), Some(&[1u8, 2, 3][..]));
    for bad in [
        // no inline copy, or one of the wrong length
        r#"{"name":"v__x","physical_name":"segments_1","length":3,"checksum":"c"}"#,
        r#"{"name":"v__x","physical_name":"segments_1","length":4,"checksum":"c","meta_hash":"AQID"}"#,
        r#"{"name":"v__x","physical_name":"segments_1","length":2,"checksum":"c","meta_hash":"AQID"}"#,
    ] {
        assert!(
            matches!(one(bad), Err(FormatError::Corrupt { .. })),
            "{bad}"
        );
    }
}

#[test]
fn an_inline_copy_is_standard_base64_and_nothing_else() {
    let inline = |text: &str| {
        one(&format!(
            r#"{{"name":"__a","physical_name":"_0.cfs","length":1,"checksum":"c","meta_hash":"{text}"}}"#
        ))
        .map(|s| s.files[0].inline.clone())
    };
    assert_eq!(inline("AQID").unwrap(), Some(vec![1, 2, 3]));
    assert_eq!(inline("AQI=").unwrap(), Some(vec![1, 2]));
    assert_eq!(inline("AQI").unwrap(), Some(vec![1, 2]));
    assert_eq!(inline("AQ==").unwrap(), Some(vec![1]));
    assert_eq!(inline("AQ").unwrap(), Some(vec![1]));
    assert_eq!(inline("").unwrap(), Some(vec![]));
    assert_eq!(inline("+/+/").unwrap(), Some(vec![0xFB, 0xFF, 0xBF]));
    for bad in ["A", "AQI!", "AQ-_", "AR==", "AQJ=", "A Q=="] {
        assert!(inline(bad).is_err(), "{bad}");
    }
    // The inline copy is bounded: 64 KiB decoded is the most, and the
    // decoded size of n unpadded characters is n * 3 / 4, rounded down.
    let chars = |n: usize| inline(&"A".repeat(n));
    assert_eq!(chars(87_382).unwrap().map(|v| v.len()), Some(65_536));
    assert!(matches!(chars(87_383), Err(FormatError::TooLarge { .. })));
}

#[test]
fn a_hostile_file_is_refused_before_a_blob_key_is_built_from_it() {
    for bad in [
        r#"{"name":"../x","physical_name":"_0.cfs","length":1,"checksum":"c"}"#,
        r#"{"name":"a/b","physical_name":"_0.cfs","length":1,"checksum":"c"}"#,
        r#"{"name":"__a","physical_name":"../_0.cfs","length":1,"checksum":"c"}"#,
        r#"{"name":"","physical_name":"_0.cfs","length":1,"checksum":"c"}"#,
        r#"{"name":"__a","physical_name":"_0.cfs","length":-1,"checksum":"c"}"#,
        r#"{"name":"__a","physical_name":"_0.cfs","length":"1","checksum":"c"}"#,
        r#"{"name":"__a","physical_name":"_0.cfs","length":1}"#,
        r#"{"name":"__a","length":1,"checksum":"c"}"#,
        r#"{"physical_name":"_0.cfs","length":1,"checksum":"c"}"#,
        r#"{"name":1,"physical_name":"_0.cfs","length":1,"checksum":"c"}"#,
    ] {
        assert!(one(bad).is_err(), "{bad}");
    }
    for bad in [
        r#"[]"#,
        r#"{"files":{}}"#,
        r#"{"files":[1]}"#,
        r#"{"name":1,"files":[]}"#,
        r#"{"name":"s","files":[]} trailing"#,
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn every_prefix_of_a_shard_snapshot_is_refused() {
    let plain = json("snapshot.plain.dat", "snapshot");
    for n in 0..plain.len() {
        assert!(
            StockShardSnapshot::parse(&plain[..n]).is_err(),
            "a prefix of {n} bytes was accepted"
        );
    }
}

#[test]
fn fields_the_reader_does_not_know_are_skipped() {
    let s = parse(
        r#"{"name":"s","index_version":4,"incremental_size":9,"extra":{"a":[1,2]},"files":[]}"#,
    )
    .expect("parses");
    assert!(s.files.is_empty());
    assert_eq!(s.name, "s");
}
