//! docs/specs/stock-snapshot-restore AC-3 and AC-9: a stock repository's
//! `index.latest`, `index-N`, snapshot, metadata and shard blobs are read
//! through a source store by derived keys, bounded; a missing or hostile one
//! is refused naming the blob.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, ReadOnlyStore, S3Op, SliceSource};
use oss_meta::{MAX_BLOB_BYTES, StockReadError, StockRepo};

const PREFIX: &str = "src/repo/";

fn fixture(dir: &str, name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../fixtures/snapshots/{dir}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    #[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    bytes
}

fn put(store: &dyn BlobStore, key: &str, bytes: &[u8]) {
    store
        .put(
            &format!("{PREFIX}{key}"),
            &mut SliceSource::new(bytes),
            &mut [0u8; 64],
        )
        .expect("put");
}

fn repo() -> (Arc<Emulator>, ReadOnlyStore) {
    let inner = Arc::new(Emulator::new(64 << 20, 1));
    put(&*inner, "index.latest", &7i64.to_be_bytes());
    put(
        &*inner,
        "index-7",
        &fixture("stock-repository-data", "index-7.json"),
    );
    put(
        &*inner,
        "snap-SnapUuid1.dat",
        &fixture("stock-metadata", "snapshot-info.ok.dat"),
    );
    put(
        &*inner,
        "indices/IdxA/meta-MetaA.dat",
        &fixture("stock-metadata", "index-metadata.plain.dat"),
    );
    put(
        &*inner,
        "indices/IdxA/0/snap-SnapUuid1.dat",
        &fixture("stock-metadata", "snapshot.deflate.dat"),
    );
    let ro = ReadOnlyStore::new(inner.clone());
    (inner, ro)
}

#[test]
fn each_blob_reads_by_its_derived_key_and_nothing_is_listed() {
    let (inner, ro) = repo();
    let r = StockRepo::new(&ro, PREFIX);
    assert_eq!(r.generation().expect("generation"), 7);
    let data = r.repository_data(7).expect("index-7");
    assert!(!data.snapshots.is_empty() && !data.indices.is_empty());
    let info = r.snapshot_info("SnapUuid1").expect("snapshot");
    assert_eq!(info.state, "SUCCESS");
    let meta = r.index_metadata("IdxA", "MetaA").expect("metadata");
    assert_eq!(meta.name, "logs");
    let shard = r.shard_snapshot("IdxA", 0, "SnapUuid1").expect("shard");
    assert_eq!(shard.files.len(), 2);
    // One GET for each blob, and no LIST.
    assert_eq!(inner.meter().count(S3Op::Get), 5);
    assert_eq!(inner.meter().count(S3Op::List), 0);
}

#[test]
fn a_missing_blob_is_named() {
    let (_, ro) = repo();
    let r = StockRepo::new(&ro, PREFIX);
    let missing = |e: StockReadError, key: &str| match e {
        StockReadError::Missing(k) => assert_eq!(k, format!("{PREFIX}{key}")),
        other => panic!("{other}"),
    };
    missing(r.repository_data(8).unwrap_err(), "index-8");
    missing(r.snapshot_info("Nope").unwrap_err(), "snap-Nope.dat");
    missing(
        r.index_metadata("IdxA", "Other").unwrap_err(),
        "indices/IdxA/meta-Other.dat",
    );
    missing(
        r.shard_snapshot("IdxA", 1, "SnapUuid1").unwrap_err(),
        "indices/IdxA/1/snap-SnapUuid1.dat",
    );
    let bare = Emulator::new(1 << 20, 1);
    missing(
        StockRepo::new(&bare, PREFIX).generation().unwrap_err(),
        "index.latest",
    );
}

#[test]
fn a_hostile_name_is_refused_before_any_request() {
    let (inner, ro) = repo();
    let r = StockRepo::new(&ro, PREFIX);
    let before = inner.meter().total();
    for bad in ["", ".", "..", "../x", "a/b", "a b", &"x".repeat(129)] {
        assert!(
            matches!(r.snapshot_info(bad), Err(StockReadError::BadName(_))),
            "{bad:?}"
        );
        assert!(matches!(
            r.index_metadata(bad, "m"),
            Err(StockReadError::BadName(_))
        ));
        assert!(matches!(
            r.index_metadata("i", bad),
            Err(StockReadError::BadName(_))
        ));
        assert!(matches!(
            r.shard_snapshot(bad, 0, "u"),
            Err(StockReadError::BadName(_))
        ));
        assert!(matches!(
            r.shard_snapshot("i", 0, bad),
            Err(StockReadError::BadName(_))
        ));
    }
    assert_eq!(inner.meter().total(), before, "no request was made");
}

#[test]
fn a_bad_generation_a_wrong_codec_and_a_corrupt_blob_are_refused_naming_the_blob() {
    let (inner, ro) = repo();
    let r = StockRepo::new(&ro, PREFIX);
    for bytes in [
        &b""[..],
        &[0u8; 7][..],
        &[0u8; 9][..],
        &(-1i64).to_be_bytes()[..],
    ] {
        put(&*inner, "index.latest", bytes);
        assert!(
            matches!(
                r.generation(),
                Err(StockReadError::Format { .. } | StockReadError::TooLarge { .. })
            ),
            "{bytes:?}"
        );
    }
    // The shard snapshot read as an index's metadata has the wrong codec.
    put(
        &*inner,
        "indices/IdxA/meta-Wrong.dat",
        &fixture("stock-metadata", "snapshot.plain.dat"),
    );
    // A flipped byte fails the footer checksum.
    let mut flipped = fixture("stock-metadata", "index-metadata.plain.dat");
    flipped[40] ^= 1;
    put(&*inner, "indices/IdxA/meta-Flip.dat", &flipped);
    for meta in ["Wrong", "Flip"] {
        match r.index_metadata("IdxA", meta).unwrap_err() {
            StockReadError::Format { key, .. } => {
                assert_eq!(key, format!("indices/IdxA/meta-{meta}.dat"));
            }
            other => panic!("{other}"),
        }
    }
    put(&*inner, "index-9", b"{ not json");
    assert!(matches!(
        r.repository_data(9),
        Err(StockReadError::Format { .. })
    ));
}

#[test]
fn a_blob_past_its_limit_is_refused() {
    let (inner, ro) = repo();
    let r = StockRepo::new(&ro, PREFIX);
    put(&*inner, "snap-Big.dat", &vec![0u8; MAX_BLOB_BYTES + 1]);
    assert!(matches!(
        r.snapshot_info("Big"),
        Err(StockReadError::TooLarge { limit, .. }) if limit == MAX_BLOB_BYTES
    ));
    put(&*inner, "snap-Edge.dat", &vec![0u8; MAX_BLOB_BYTES]);
    assert!(
        matches!(r.snapshot_info("Edge"), Err(StockReadError::Format { .. }),),
        "a blob at the limit is read, and refused for what it holds"
    );
}
