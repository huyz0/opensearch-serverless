//! A log the shell's own committer wrote on a filesystem store replays
//! here as the shell's own takeover read it (spec node-log-replay, AC-6).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oss_blob::{BlobStore, FsStore};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_testkit::TempDir;
use oss_writer::log::{NodeLogStore, NodeLogTakeover, TakeoverError};

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
