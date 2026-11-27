//! Publishing into node packs over the in-process emulator: the requests a
//! publish makes with packs on, where each file lands, and the swap deadline.

use std::sync::Arc;
use std::time::Duration;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, ByteRange, Emulator, S3Op};
use oss_format::{CommitManifest, FileLocation, ShardRef, node_pack_key};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::pack::{PackLedger, PackStore, PackWriter};
use oss_writer::publish::{PackPlacement, PublishError, ShardPublisher};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};
const NODE: &str = "node-1";

fn shard(dir: &ByteBuffersDirectory) -> Shard<'_> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    Shard::new(dir, w, mapping, 3, Box::new(|| 0)).expect("shard")
}

fn commit(s: &mut Shard<'_>, id: &str) -> Vec<String> {
    s.index(id, br#"{"n":1}"#, false, &Expect::default())
        .expect("indexed");
    s.refresh().expect("refresh");
    s.committed_files().expect("files")
}

fn store() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

fn pack_writer(e: &Arc<Emulator>, linger_ms: u64) -> Arc<PackWriter> {
    Arc::new(PackWriter::new(
        PackStore::new(dynstore(e), "", NODE),
        Some(PackLedger::new(dynstore(e), "", NODE).expect("ledger")),
        11,
        Duration::from_millis(linger_ms),
        64 << 20,
    ))
}

fn publisher(
    e: &Arc<Emulator>,
    index: &str,
    number: u32,
    packs: Option<Arc<PackWriter>>,
    max: u64,
) -> ShardPublisher {
    let shard = ShardRef {
        index,
        uuid: &format!("{index}-uuid"),
        shard: number,
    };
    let mut p = ShardPublisher::new(dynstore(e), "", &shard).expect("publisher");
    p.use_packs(packs.map(|writer| PackPlacement {
        writer,
        index_name: index.to_owned(),
        index_uuid: format!("{index}-uuid"),
        shard: number,
        max_blob_bytes: max,
    }));
    p
}

fn read(e: &Emulator, key: &str, range: ByteRange) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    e.get(
        key,
        range,
        &mut buf,
        &mut |c: &[u8]| -> oss_blob::BlobResult<()> {
            out.extend_from_slice(c);
            Ok(())
        },
    )
    .expect("reads");
    out
}

fn file_bytes(dir: &ByteBuffersDirectory, name: &str) -> Vec<u8> {
    use lucene_store::Directory;
    let input = dir.open(name).expect("opens");
    (*input).to_vec()
}

/// AC-1: with packs on, a refresh is one pack and one swap, no blob of its
/// own, and each manifest location holds its file's bytes.
#[test]
fn a_publish_into_a_pack_is_one_pack_put_and_one_swap() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let e = store();
    let mut p = publisher(&e, "logs", 0, Some(pack_writer(&e, 1)), 8 << 20);
    let files = commit(&mut s, "a");
    let published = p.publish(&dir, &files, 3, NODE, 0, 1).expect("published");
    // The pack, the ledger's create, and the register swap's create.
    assert_eq!(e.meter().count(S3Op::Put), 3);
    assert!(published.blobs.is_empty(), "no blob of its own");
    let manifest = CommitManifest::parse(&published.manifest).expect("manifest");
    assert!(!manifest.files().is_empty());
    for file in manifest.files() {
        let FileLocation::NodePacked {
            node,
            epoch,
            ordinal,
            offset,
            length,
        } = file.location
        else {
            panic!("{} is {:?}", file.name, file.location);
        };
        let key = node_pack_key("", node, epoch, ordinal).unwrap();
        let bytes = read(
            &e,
            &key,
            ByteRange::From {
                start: offset,
                len: length.max(1),
            },
        );
        assert_eq!(bytes, file_bytes(&dir, &file.name), "{}", file.name);
    }
}

/// AC-2: two shards publishing within the linger share one pack.
#[test]
fn two_shards_publishing_together_share_one_pack() {
    let e = store();
    let writer = pack_writer(&e, 400);
    let handles: Vec<_> = (0..2u32)
        .map(|n| {
            let e = Arc::clone(&e);
            let writer = Arc::clone(&writer);
            std::thread::spawn(move || {
                let dir = ByteBuffersDirectory::new();
                let mut s = shard(&dir);
                let mut p = publisher(&e, &format!("idx{n}"), 0, Some(writer), 8 << 20);
                let files = commit(&mut s, "a");
                p.publish(&dir, &files, 3, NODE, 0, 1)
                    .expect("published")
                    .manifest
            })
        })
        .collect();
    let manifests: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let mut packs = std::collections::BTreeSet::new();
    for m in &manifests {
        for f in CommitManifest::parse(m).unwrap().files() {
            if let FileLocation::NodePacked { epoch, ordinal, .. } = f.location {
                packs.insert((epoch, ordinal));
            }
        }
    }
    assert_eq!(packs.len(), 1, "one pack for both shards: {packs:?}");
}

/// AC-3: a blob over the limit keeps its own blob; with packs off nothing is
/// packed; switching off again goes back to blobs.
#[test]
fn a_large_blob_and_a_publish_without_packs_keep_their_own_blobs() {
    for (packs_on, max) in [(true, 1u64), (false, 8 << 20)] {
        let dir = ByteBuffersDirectory::new();
        let mut s = shard(&dir);
        let e = store();
        let mut p = publisher(&e, "logs", 0, packs_on.then(|| pack_writer(&e, 1)), max);
        let files = commit(&mut s, "a");
        let published = p.publish(&dir, &files, 3, NODE, 0, 1).expect("published");
        assert_eq!(published.blobs.len(), 1, "packs_on={packs_on}");
        assert_eq!(
            e.meter().count(S3Op::Put),
            2,
            "the blob and the swap; no pack, no ledger"
        );
    }
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let e = store();
    let mut p = publisher(&e, "logs", 0, Some(pack_writer(&e, 1)), 8 << 20);
    p.publish(&dir, &commit(&mut s, "a"), 3, NODE, 0, 1)
        .unwrap();
    p.use_packs(None);
    let second = p
        .publish(&dir, &commit(&mut s, "b"), 3, NODE, 1, 2)
        .unwrap();
    assert_eq!(second.blobs.len(), 1, "back to a blob of its own");
}

/// AC-4: a swap long after its pack landed is refused and writes no register;
/// the next publish packs the files anew.
#[test]
fn a_swap_past_the_deadline_is_refused_and_the_next_publish_repacks() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let e = store();
    let mut p = publisher(&e, "logs", 0, Some(pack_writer(&e, 1)), 8 << 20);
    p.set_swap_deadline(Duration::ZERO);
    let files = commit(&mut s, "a");
    let refused = p.publish(&dir, &files, 3, NODE, 0, 1);
    assert!(
        matches!(refused, Err(PublishError::PackExpired)),
        "{refused:?}"
    );
    let register = oss_format::ShardRef {
        index: "logs",
        uuid: "logs-uuid",
        shard: 0,
    }
    .manifest_key("")
    .unwrap();
    let mut buf = [0u8; 16];
    assert!(
        e.read_register(&register, &mut buf).unwrap().is_none(),
        "no register written"
    );
    p.set_swap_deadline(Duration::from_secs(300));
    let ok = p.publish(&dir, &files, 3, NODE, 0, 2).expect("published");
    let manifest = CommitManifest::parse(&ok.manifest).unwrap();
    assert!(
        manifest
            .files()
            .iter()
            .all(|f| matches!(f.location, FileLocation::NodePacked { .. }))
    );
}
