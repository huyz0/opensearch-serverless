//! A log the shell's own committer wrote on a filesystem store replays
//! here as the shell's own takeover read it, and the Rust committer writes
//! the same log (spec node-log-replay, AC-6).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, FsStore};
use oss_format::{NodeLogEpoch, ShardKey, WalRecord};
use oss_testkit::TempDir;
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, NodeLogTakeover, Refusal,
    ShardAdmission, TakeoverError,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/node-log-replay")
}

/// A small fixture file, read whole.
#[allow(clippy::disallowed_methods)]
fn lines(name: &str) -> Vec<String> {
    fs::read_to_string(fixture().join(name))
        .expect("fixture")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for entry in fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// Every file under `root`, by its key; the log is a few blobs of a few KiB.
#[allow(clippy::disallowed_methods)]
fn tree(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("entry");
        if entry.file_type().expect("type").is_dir() {
            tree(root, &entry.path(), out);
        } else {
            let key = entry
                .path()
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(key, fs::read(entry.path()).expect("read"));
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn epochs() -> Vec<NodeLogEpoch> {
    lines("lease.txt")
        .iter()
        .map(|line| {
            let n: Vec<i64> = line
                .split(' ')
                .map(|part| part.parse().expect("number"))
                .collect();
            NodeLogEpoch {
                epoch: n[0],
                trim: n[1],
                landed: n[2],
            }
        })
        .collect()
}

fn shard(line: &str) -> ShardKey {
    let parts: Vec<&str> = line.split(' ').collect();
    ShardKey::new(parts[1], parts[2].parse().expect("shard"), Some(parts[3]))
}

#[test]
fn a_log_the_shell_wrote_on_fs_replays_as_the_shell_reads_it() {
    let dir = TempDir::new("node-log-cross-replay");
    copy(&fixture().join("log"), dir.path());
    let mut before = BTreeMap::new();
    tree(dir.path(), dir.path(), &mut before);
    assert!(!before.is_empty(), "the recorded log is there");

    let store: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()).expect("open"));
    let log = NodeLogStore::new(store, "", "node-1").expect("log");
    let takeover = NodeLogTakeover::new(log, epochs(), "node-2", 2);
    takeover.fence_all().expect("fences");

    let mut got = Vec::new();
    for (epoch, end) in takeover.ends().expect("ends") {
        got.push(format!("end {epoch} {end}"));
    }
    for line in lines("replay.txt")
        .iter()
        .filter(|l| l.starts_with("shard "))
    {
        let key = shard(line);
        got.push(line.clone());
        got.push(format!(
            "max {}",
            takeover.max_seq_no_of(&key).expect("max")
        ));
        got.push(format!("holds {}", takeover.holds(&key).expect("holds")));
        takeover
            .for_each_record(&key, |record| {
                let encoded = record.encode().expect("encodes");
                let json = String::from_utf8(encoded.bytes().to_vec()).expect("utf-8");
                got.push(format!("record {json}"));
                Ok::<_, TakeoverError>(())
            })
            .expect("reads");
    }
    assert_eq!(got, lines("replay.txt"));

    let mut after = BTreeMap::new();
    tree(dir.path(), dir.path(), &mut after);
    let written: Vec<String> = after
        .iter()
        .filter(|(key, _)| !before.contains_key(*key))
        .map(|(key, bytes)| format!("{key} {}", hex(bytes)))
        .collect();
    assert_eq!(
        written,
        lines("fences.txt"),
        "the same fence at the same key"
    );
}

struct Lease(Mutex<Vec<NodeLogEpoch>>);

impl EpochLease for Lease {
    fn record(&self, epochs: &[NodeLogEpoch]) -> Result<(), Refusal> {
        *self.0.lock().expect("unpoisoned") = epochs.to_vec();
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

fn doc(id: &str, seq: i64, source: &str) -> WalRecord<'static> {
    WalRecord::index(
        id.to_owned(),
        source.to_owned(),
        seq,
        1,
        seq.saturating_add(1),
    )
}

fn key(uuid: &str, shard: u32, name: &str) -> ShardKey {
    ShardKey::new(uuid, shard, Some(name))
}

/// The calls `NodeLogReplay.java` makes, made on the Rust committer: it
/// writes the same blobs at the same keys, so the shell replays a log the
/// Rust writer wrote as it replays its own.
#[test]
fn the_rust_committer_writes_the_log_the_shell_wrote() {
    let dir = TempDir::new("node-log-cross-write");
    let store: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()).expect("open"));
    let log = NodeLogStore::new(store, "", "node-1").expect("log");
    let committer = NodeLogCommitter::new(
        log.clone(),
        7,
        Box::new(Lease(Mutex::new(Vec::new()))),
        Box::new(Admit),
        CommitterConfig {
            window: Duration::ZERO,
            max_bytes: 1 << 20,
            max_records: 1000,
        },
    );
    let a = key("logs-uuid-000000000000", 0, "logs");
    let b = key("logs-uuid-000000000000", 1, "logs");
    let c = key("metrics-uuid-0000000000", 0, "metrics");

    committer
        .commit(&a, 1, &[doc("a0", 0, "{\"n\":0}")])
        .expect("lands");
    let b0 = committer
        .commit(&b, 1, &[doc("b0", 0, "{\"n\":\"b\"}")])
        .expect("lands")
        .expect("some");
    let a1 = Arc::new(Mutex::new(None));
    let landed = Arc::clone(&a1);
    committer
        .enqueue(&a, 1, &[doc("a1", 1, "{\"n\":1}")], move |at| {
            *landed.lock().expect("unpoisoned") = Some(at);
        })
        .expect("joins");
    committer
        .enqueue(
            &c,
            1,
            &[
                doc("c0", 0, "{\"m\":[1,2]}"),
                WalRecord::deletion("c0", 1, 1, 2),
            ],
            |_| {},
        )
        .expect("joins");
    committer
        .enqueue(&b, 1, &[doc("b1", 1, "{\"n\":\"b1\"}")], |_| {})
        .expect("joins");
    committer.flush_queued();
    log.fence(7, 0, "someone", 1).expect("fences");
    assert!(committer.commit(&a, 1, &[doc("refused", 2, "{}")]).is_err());
    let unicode = "{\"t\":\"caf\u{e9} \u{1F600} \\\"q\\\" \\n\"}";
    committer
        .commit(&a, 1, &[doc("a2", 2, unicode)])
        .expect("lands");
    let pad = format!("{{\"pad\":\"{}\"}}", "x".repeat(5000));
    committer
        .commit(&b, 1, &[doc("b2", 2, &pad)])
        .expect("lands");
    committer
        .commit(&c, 2, &[doc("c2", 2, "{\"m\":3}")])
        .expect("lands");
    let a1 = a1.lock().expect("unpoisoned").expect("a1 landed");
    committer.on_published(&a, a1);
    committer.on_published(&b, b0);

    let mut wrote = BTreeMap::new();
    tree(dir.path(), dir.path(), &mut wrote);
    let mut recorded = BTreeMap::new();
    let fixture_log = fixture().join("log");
    tree(&fixture_log, &fixture_log, &mut recorded);
    assert_eq!(
        wrote.keys().collect::<Vec<_>>(),
        recorded.keys().collect::<Vec<_>>(),
        "the same keys"
    );
    for (key, bytes) in &recorded {
        assert!(wrote.get(key) == Some(bytes), "{key}: the same bytes");
    }
    let lease: Vec<String> = committer
        .positions()
        .iter()
        .map(|e| format!("{} {} {}", e.epoch, e.trim, e.landed))
        .collect();
    assert_eq!(lease, lines("lease.txt"));
}
