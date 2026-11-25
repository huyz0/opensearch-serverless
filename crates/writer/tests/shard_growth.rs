//! docs/specs/dynamic-mapping-durable: what a shard's writes add to its mapping.

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

// docs/specs/dynamic-mapping-durable AC-1
#[test]
fn a_shard_hands_over_what_its_writes_added_to_the_mapping() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    s.index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert!(s.take_growth().is_none(), "n was mapped already");
    s.index("b", br#"{"t":"x"}"#, false, &Expect::default())
        .unwrap();
    s.index("c", br#"{"u":true}"#, false, &Expect::default())
        .unwrap();
    let grown = s.take_growth().expect("two fields were added").to_json();
    assert!(grown.contains(r#""t":{"type":"text""#), "{grown}");
    assert!(grown.contains(r#""u":{"type":"boolean""#), "{grown}");
    assert!(!grown.contains(r#""n":"#), "{grown}");
    assert!(s.take_growth().is_none(), "taken once");
}

fn shard(dir: &ByteBuffersDirectory) -> Shard<'_> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    Shard::new(dir, w, mapping, 3, Box::new(|| 0)).expect("shard")
}
