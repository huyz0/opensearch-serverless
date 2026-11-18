//! Writes a small shard through the Rust writer and publishes it twice to a
//! directory, for a Java reader to open (`fixtures/exit-gate/README.md`).
//!
//! `cargo run -p oss-node --example write_commit -- <out dir>` leaves
//! `<out>/store/` (the blobs as the object store holds them), `<out>/manifest.bin`
//! (the last commit register's value). The live documents are `a` (written
//! again) and `c`; `b` is deleted.

use std::path::PathBuf;
use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore};
use oss_format::ShardRef;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("an output directory"));
    std::fs::create_dir_all(&out).expect("creates the directory");
    let store = Arc::new(FsStore::new(out.join("store")).expect("store"));
    // The index's descriptor, as the shell writes it.
    store
        .create_register(
            "indices/logs",
            br#"{"name":"logs","uuid":"u1","number_of_shards":1,"mapping":"{\"properties\":{\"n\":{\"type\":\"integer\"},\"title\":{\"type\":\"text\"},\"tag\":{\"type\":\"keyword\"}}}","settings":{}}"#,
        )
        .expect("descriptor");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(
        br#"{"properties":{"n":{"type":"integer"},"title":{"type":"text"},"tag":{"type":"keyword"}}}"#,
    )
    .expect("mapping");
    let mut shard = Shard::new(&dir, w, mapping, 3, Box::new(|| 0)).expect("shard");
    let shard_ref = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut publisher =
        ShardPublisher::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", &shard_ref)
            .expect("publisher");
    let mut manifest = Vec::new();
    for (ordinal, step) in [0u32, 1].into_iter().enumerate() {
        if step == 0 {
            for (id, text) in [("a", "red fox"), ("b", "blue whale"), ("c", "green frog")] {
                let body = format!(r#"{{"n":1,"title":"{text}","tag":"{id}"}}"#);
                shard
                    .index(id, body.as_bytes(), false, &Expect::default())
                    .expect("indexed");
            }
        } else {
            shard
                .index(
                    "a",
                    br#"{"n":2,"title":"red fox again","tag":"a"}"#,
                    false,
                    &Expect::default(),
                )
                .expect("replaced");
            shard.delete("b", &Expect::default()).expect("deleted");
        }
        shard.refresh().expect("refresh");
        let files = shard.committed_files().expect("files");
        let ordinal = i64::try_from(ordinal).expect("small");
        manifest = publisher
            .publish(
                &dir,
                &files,
                3,
                "node-1",
                ordinal,
                u64::try_from(ordinal).expect("small"),
            )
            .expect("published")
            .manifest;
    }
    std::fs::write(out.join("manifest.bin"), &manifest).expect("writes the manifest");
}
