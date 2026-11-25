//! A Rust writer takes over a shard the Java shell wrote (fixtures/shell-blobs).

// ARITH: fixture lengths and small counts.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

mod proactive_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oss_blob::{BlobStore, SliceSource};
use oss_cluster::heads::shard_head_key;
use oss_format::{CommitManifest, ShardHead};
use oss_node::writer::Takeover;
use oss_writer::takeover::{HeadWriters, Predecessors};
use proactive_support::*;

const INDEX: &str = "fixture";
const UUID: &str = "fixture-uuid-0000000";

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).expect("fixture dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.file_name().is_some_and(|n| n != "README.md") {
            let key = path
                .strip_prefix(root)
                .expect("under")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((key, std::fs::read(&path).expect("read")));
        }
    }
}

/// The shell's blobs in the emulator, registers by their value (the first
/// eight bytes of a fixture register are its generation).
fn load(c: &Cluster) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/shell-blobs");
    let mut blobs = Vec::new();
    walk(&root, &root, &mut blobs);
    for (key, bytes) in blobs {
        if key.starts_with("indices/") || key.ends_with("/manifest") {
            c.store
                .create_register(&key, &bytes[8..])
                .expect("register");
        } else {
            c.store
                .put(&key, &mut SliceSource::new(&bytes), &mut [0u8; 1 << 16])
                .expect("blob");
        }
    }
    let head = ShardHead {
        index: INDEX.to_owned(),
        shard: 0,
        term: 2,
        owner_node_id: Some("gnPAlf__T_-qxystAAAAAA".to_owned()),
        owner_ephemeral_id: Some("java".to_owned()),
        lease_expires_at_millis: 0,
        index_uuid: Some(UUID.to_owned()),
        writers: Vec::new(),
    };
    c.store
        .create_register(
            &shard_head_key("", INDEX, 0),
            &head.to_bytes().expect("bytes"),
        )
        .expect("head");
}

/// The commit register's value as text, and its term.
fn manifest(c: &Cluster) -> (u64, String) {
    let mut buf = vec![0u8; 1 << 16];
    let key = format!("segments/{INDEX}#{UUID}#0/manifest");
    let read = c
        .store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("there");
    let value = &buf[..read.len];
    let term = CommitManifest::parse(value).expect("parses").term;
    (term, String::from_utf8_lossy(value).into_owned())
}

/// Copies the store to `dir` as the filesystem store lays it out, for the
/// Java recorder (`fixtures/mixed-takeover`): registers are an 8-byte
/// generation then the value.
fn dump(c: &Cluster, dir: &Path) {
    let mut keys = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut page = Vec::new();
        let done = c
            .store
            // LIST: a test copying the whole emulator store out for the recorder.
            .list_prefix("", after.as_deref(), 1000, &mut |k, _| {
                page.push(k.to_owned())
            })
            .expect("lists");
        after = page.last().cloned();
        keys.extend(page);
        if !done.truncated {
            break;
        }
    }
    for key in keys {
        let register =
            key.starts_with("indices/") || key.starts_with("shards/") || key.ends_with("/manifest");
        let mut bytes = Vec::new();
        if register {
            let mut buf = vec![0u8; 1 << 16];
            let read = c
                .store
                .read_register(&key, &mut buf)
                .expect("reads")
                .expect("there");
            bytes.extend_from_slice(&read.generation.to_be_bytes());
            bytes.extend_from_slice(&buf[..read.len]);
        } else {
            let mut chunk = vec![0u8; 1 << 16];
            c.store
                .get(
                    &key,
                    oss_blob::ByteRange::Full,
                    &mut chunk,
                    &mut |b: &[u8]| {
                        bytes.extend_from_slice(b);
                        Ok(())
                    },
                )
                .expect("gets");
        }
        let path = dir.join(&key);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, bytes).expect("written");
    }
}

fn nobody(c: &Cluster) -> Takeover {
    Takeover {
        predecessors: Arc::new(Predecessors::new(Box::new(|_| Ok(None)))),
        heads: HeadWriters::new(c.store.clone(), ""),
        max_buffer_bytes: 1 << 20,
    }
}

#[test]
fn a_rust_writer_takes_a_java_shard_and_keeps_its_blobs() {
    let c = cluster();
    load(&c);
    let (term, before) = manifest(&c);
    assert_eq!(term, 2);
    assert!(
        before.contains("e=00000001794710559389") && before.contains("t=2/_1.seg"),
        "{before}"
    );
    let rust = node(&c, "node-2", 3, true, Some(nobody(&c)));
    let result = write_in(&rust.role, INDEX, "from-rust");
    assert_eq!(seq(&result), 5, "after the Java's five documents");
    let head = {
        let mut buf = vec![0u8; 1 << 16];
        let read = c
            .store
            .read_register(&shard_head_key("", INDEX, 0), &mut buf)
            .expect("reads")
            .expect("there");
        ShardHead::parse(&buf[..read.len]).expect("head")
    };
    assert_eq!(head.term, 3, "the take bumps the term");
    assert_eq!(head.owner_node_id.as_deref(), Some("node-2"));
}

fn info() -> oss_writer::bulk::IndexInfo {
    oss_writer::bulk::IndexInfo {
        name: INDEX.into(),
        uuid: UUID.into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

#[test]
fn the_commit_a_rust_writer_publishes_still_names_the_java_blobs() {
    let c = cluster();
    load(&c);
    let rust = node(&c, "node-2", 3, true, Some(nobody(&c)));
    seq(&write_in(&rust.role, INDEX, "from-rust"));
    rust.role.refresh(&info(), 0).expect("published");
    let (term, after) = manifest(&c);
    assert_eq!(term, 3);
    // Set to write the store the Java recorder reads (fixtures/mixed-takeover).
    if let Some(out) = std::env::var_os("OSS_MIXED_OUT") {
        dump(&c, Path::new(&out));
    }
    assert!(
        after.contains("e=00000001794710559389"),
        "the Java node pack: {after}"
    );
    assert!(
        after.contains("t=2/_1.seg"),
        "the Java segment blob: {after}"
    );
}

#[test]
fn a_live_janitor_sweep_after_the_take_deletes_no_blob_the_commit_names() {
    use std::sync::atomic::{AtomicU64, Ordering};

    use oss_janitor::gc::Config;
    use oss_mem::{BudgetConfig, MemoryBudget, Pool};
    use oss_meta::PointInTimeStore;
    use oss_node::writer::{JanitorPass, PassLimits};

    let c = cluster();
    load(&c);
    let rust = node(&c, "node-2", 3, true, Some(nobody(&c)));
    seq(&write_in(&rust.role, INDEX, "from-rust"));
    rust.role.refresh(&info(), 0).expect("published");
    let orphan = format!("segments/{INDEX}#{UUID}#0/t=1/orphan");
    c.store
        .put(&orphan, &mut SliceSource::new(b"old"), &mut [0u8; 16])
        .expect("planted");
    let clock = Arc::new(AtomicU64::new(1_000));
    let read = Arc::clone(&clock);
    let pits = PointInTimeStore::new(
        c.store.clone() as Arc<dyn BlobStore>,
        "",
        Arc::new(move || read.load(Ordering::SeqCst)),
    )
    .expect("pits");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::RequestBodies, 1 << 20)
            .expect("fits"),
    );
    let pass = JanitorPass::new(
        c.store.clone() as Arc<dyn BlobStore>,
        "",
        Arc::clone(&rust.role),
        pits,
        &budget,
        PassLimits {
            buffer_bytes: 1 << 16,
            max_views: 100,
        },
        Box::new(|| 5_000),
    );
    let live = Config {
        dry_run: false,
        minimum_unreferenced_millis: 0,
        max_watched: 1000,
        max_pinned: 1000,
        relist_after_millis: 0,
    };
    pass.run(live);
    // A second sweep deletes what the first saw unreferenced.
    seq(&write_in(&rust.role, INDEX, "again"));
    rust.role.refresh(&info(), 0).expect("published again");
    pass.run(live);
    let present = |key: &str| c.store.head(key).expect("heads").is_some();
    assert!(
        !present(&orphan),
        "the planted orphan is swept: the pass ran for real"
    );
    assert!(
        present(&format!("segments/{INDEX}#{UUID}#0/t=2/_1.seg")),
        "the Java segment blob"
    );
    assert!(
        present("packs/tZ9iGQAAQACwLK7Z_____w/e=00000001794710559389/00000000000000000000"),
        "the Java node pack"
    );
}
