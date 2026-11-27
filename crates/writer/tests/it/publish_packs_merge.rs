//! A merging shard published into node packs: every file the commit names
//! lands in a pack with the shard's own bytes, round after round.

use std::sync::Arc;
use std::time::Duration;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, ByteRange, Emulator};
use oss_format::{CommitManifest, FileLocation, ShardRef, node_pack_key};
use oss_writer::index::{Expect, MergeGate, MergeGuard, MergeSettings, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::pack::{PackLedger, PackStore, PackWriter};
use oss_writer::publish::{PackPlacement, ShardPublisher};

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
    Shard::new(dir, w, mapping, 3, Box::new(|| 0))
        .expect("shard")
        .with_merging(
            MergeSettings {
                segments_per_tier: 2,
                max_merge_at_once: 4,
                max_merged_segment_bytes: 64 << 20,
            },
            Arc::new(Open),
        )
}

struct Open;

impl MergeGate for Open {
    fn try_start(&self, _: u64) -> Option<Box<dyn MergeGuard>> {
        Some(Box::new(()))
    }
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

// docs/specs/writer-segment-merge AC-3
#[test]
fn a_merging_shard_publishes_every_file_of_every_commit_into_packs() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let e = store();
    let mut p = publisher(&e, "logs", 0, Some(pack_writer(&e, 1)), 8 << 20);
    for round in 0..20u64 {
        let files = commit(&mut s, &round.to_string());
        let published = p
            .publish(
                &dir,
                &files,
                3,
                NODE,
                i64::try_from(round).unwrap(),
                round + 1,
            )
            .expect("published");
        assert!(published.blobs.is_empty(), "no blob of its own");
        let manifest = CommitManifest::parse(&published.manifest).expect("manifest");
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
        assert_eq!(manifest.files().len(), files.len(), "round {round}");
    }
    use lucene_search::directory_reader::DirectoryReader;
    let segments = DirectoryReader::open(&dir)
        .expect("open")
        .segment_readers()
        .len();
    assert!(segments < 20, "merging ran: {segments} segments");
}
