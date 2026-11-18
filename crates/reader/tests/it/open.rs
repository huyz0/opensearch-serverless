//! Opening the shell's published commit.

use oss_blob::S3Op;
use oss_reader::{ReaderError, ShardFiles, open_shard};

use crate::{BASE, INDEX, cache, counts, since, store_with_fixtures};

#[test]
fn the_commit_is_found_with_two_gets_and_no_list() {
    let store = store_with_fixtures();
    let before = counts(&store);
    let files = ShardFiles::open(&store, BASE, INDEX, 0).expect("open");
    assert_eq!(since(&store, before), [(S3Op::Get, 2)]);
    assert_eq!(files.term, 2);
    let names: Vec<&str> = files.names().collect();
    assert_eq!(
        names,
        [
            "_0.cfe",
            "_0.cfs",
            "_0.si",
            "_1.cfe",
            "_1.cfs",
            "_1.si",
            "segments_5"
        ]
    );
    let si = files.get("_0.si").expect("_0.si");
    assert!(
        si.key.starts_with("packs/"),
        "segment _0 lives in the node pack: {}",
        si.key
    );
    let si1 = files.get("_1.si").expect("_1.si");
    assert!(
        si1.key.ends_with("/t=2/_1.seg"),
        "segment _1 lives in its segment blob: {}",
        si1.key
    );
}

#[test]
fn the_engine_opens_both_segments_with_ranged_reads_only() {
    let store = store_with_fixtures();
    let before = counts(&store);
    let shard = open_shard(store.clone(), cache(), BASE, INDEX, 0).expect("open shard");
    // Three documents in the node pack's segment, two in the segment blob's.
    assert_eq!(shard.reader.max_doc(), 5);
    assert_eq!(shard.reader.num_docs(), 5);
    assert_eq!(
        format!("{shard:?}"),
        "OpenShard { max_doc: 5, num_docs: 5 }"
    );
    let made = since(&store, before);
    let count = |op: S3Op| made.iter().find(|(o, _)| *o == op).map_or(0, |(_, n)| *n);
    assert_eq!(
        count(S3Op::Get),
        2,
        "only the two registers are read whole: {made:?}"
    );
    assert!(
        count(S3Op::GetRange) >= 1,
        "segment files come by range: {made:?}"
    );
    for op in [S3Op::List, S3Op::Head, S3Op::Put, S3Op::Copy, S3Op::Delete] {
        assert_eq!(count(op), 0, "{op:?}: {made:?}");
    }
}

#[test]
fn a_missing_index_is_not_found_after_one_get() {
    let store = store_with_fixtures();
    let before = counts(&store);
    let err = ShardFiles::open(&store, BASE, "no-such-index", 0).expect_err("missing");
    assert!(
        matches!(err, ReaderError::Blob(oss_blob::BlobError::NotFound)),
        "{err}"
    );
    assert_eq!(since(&store, before), [(S3Op::Get, 1)]);
}

#[test]
fn the_engine_store_lists_sizes_and_versions_from_the_manifest_alone() {
    use lucene_store::BlobStore as _;
    let store = store_with_fixtures();
    let files = ShardFiles::open(&store, BASE, INDEX, 0).expect("open");
    let engine = oss_reader::EngineStore::new(store.clone(), files.clone());
    let before = counts(&store);
    // LIST: the engine's listing, answered from the manifest; no request is sent.
    assert_eq!(engine.list().expect("list").len(), files.len());
    let sizes = engine
        .list_with_sizes()
        .expect("sizes")
        .expect("lengths known");
    assert_eq!(sizes.len(), files.len());
    let si = files.get("_0.si").expect("_0.si");
    assert_eq!(engine.size("_0.si").expect("size"), si.length);
    assert_eq!(
        engine.version("_0.si").expect("version"),
        Some(si.key.clone())
    );
    assert!(engine.exists("_0.si").expect("exists"));
    assert!(!engine.exists("_9.si").expect("exists"));
    assert!(engine.size("_9.si").expect_err("missing").is_no_such_file());
    assert_eq!(since(&store, before), [], "answered from the manifest");
    assert!(engine.put("x", vec![1]).is_err(), "read-only");
    assert!(engine.delete("_0.si").is_err(), "read-only");
    assert_eq!(engine.files().len(), files.len());
}

#[test]
fn a_whole_file_read_is_one_ranged_get_of_exactly_that_file() {
    use lucene_store::BlobStore as _;
    let store = store_with_fixtures();
    let files = ShardFiles::open(&store, BASE, INDEX, 0).expect("open");
    let engine = oss_reader::EngineStore::new(store.clone(), files.clone());
    let before = counts(&store);
    let lucene_store::Input::Owned(bytes) = engine.get("_1.si").expect("get") else {
        panic!("an owned read");
    };
    assert_eq!(
        bytes.len() as u64,
        files.get("_1.si").expect("_1.si").length
    );
    // Lucene's codec header magic, 0x3fd76c17.
    assert_eq!(&bytes[..4], &[0x3f, 0xd7, 0x6c, 0x17]);
    assert_eq!(since(&store, before), [(S3Op::GetRange, 1)]);
    let mut out = [0u8; 8];
    let len = files.get("_1.si").expect("_1.si").length;
    assert_eq!(
        engine
            .get_range_into("_1.si", len, &mut out)
            .expect("at end"),
        0
    );
    assert_eq!(
        engine
            .get_range_into("_1.si", len - 3, &mut out)
            .expect("tail"),
        3
    );
}

#[test]
fn the_mapping_source_is_kept_for_the_overlay_s_builder() {
    let store = store_with_fixtures();
    let (_files, meta) = ShardFiles::open_with_meta(&store, BASE, INDEX, 0).expect("meta");
    assert!(
        meta.mapping_json.starts_with('{') && meta.mapping_json.contains("properties"),
        "{}",
        meta.mapping_json
    );
    oss_doc::mapping::Mapping::parse(meta.mapping_json.as_bytes())
        .expect("parses as the builder reads it");
}
