//! docs/specs/stock-snapshot-export AC-7, AC-8 and AC-9: a take into a stock
//! repository over REST, refused where the spec says before anything is
//! written, and kept restorable by a second take.

// The source is the recorded shard's files; the target is a temporary directory.
#![allow(clippy::disallowed_methods)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, SliceSource, SourceRoots};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::snapshot_route::{SnapshotService, routes};
use oss_node::snapshot_take_route::SnapshotTakeService;
use oss_node::stock_export_route::StockExportSettings;
use oss_rest::{Router, Server};

use crate::support;

const SOURCE: &str = "../../fixtures/snapshots/stock/repo/indices/M3IuI3tpQ7mcpIwUIA1kfQ/0/";
const FILES: [&str; 4] = ["_0.si", "_0.cfs", "_0.cfe", "segments_1"];

pub(crate) struct Rig {
    pub(crate) server: Server,
    pub(crate) store: Arc<Emulator>,
    pub(crate) root: PathBuf,
    descriptors: DescriptorStore,
}

fn recorded(file: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(SOURCE)
            .join(file),
    )
    .unwrap_or_default()
}

/// The recorded shard's files, by physical name and bytes, read as the
/// restore's fixture lists them.
fn shard_files() -> Vec<(String, Vec<u8>)> {
    let mut json = Vec::new();
    oss_format::stock_blob_to_json(
        &recorded("snap-q6tlFJQcRUS97oE_eOaE7Q.dat"),
        "snapshot",
        &oss_format::StockEnvelopeLimits::default(),
        &mut json,
    )
    .unwrap();
    let snap = oss_format::StockShardSnapshot::parse(&json).unwrap();
    snap.files
        .iter()
        .map(|f| {
            let bytes = f.inline.clone().unwrap_or_else(|| recorded(&f.name));
            (f.physical_name.clone(), bytes)
        })
        .collect()
}

fn rig(enabled: bool, tag: &str) -> Rig {
    let root = std::env::temp_dir().join(format!("oss-export-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("out")).unwrap();
    let root = root.canonicalize().unwrap();
    let settings = StockExportSettings {
        enabled,
        roots: SourceRoots::parse(root.to_str().unwrap(), ""),
        s3: None,
        chunk_bytes: 1 << 16,
    };
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        root.join("out").display()
    );
    rig_with(settings, root, &body)
}

/// A node whose export goes as `settings` say, with the repository `stock` registered as `repo`.
pub(crate) fn rig_with(settings: StockExportSettings, root: PathBuf, repo: &str) -> Rig {
    stand_up(settings, root, repo, 200 << 20)
}

fn stand_up(settings: StockExportSettings, root: PathBuf, repo: &str, pool: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
    let descriptors =
        DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
    let mut record = IndexRecord::new("logs", "u1", 1);
    record.mapping = Some(
        r#"{"properties":{"title":{"type":"text"},"level":{"type":"keyword"},"n":{"type":"integer"}}}"#
            .into(),
    );
    descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .unwrap();
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let dir = format!("{}t=1/", at.base_path("").unwrap());
    let mut entries = Vec::new();
    let files = shard_files();
    for (name, bytes) in &files {
        store
            .put(
                &format!("{dir}{name}"),
                &mut SliceSource::new(bytes),
                &mut [0u8; 64],
            )
            .unwrap();
    }
    for (name, bytes) in &files {
        entries.push(ManifestEntry {
            name,
            location: FileLocation::Whole { term_dir: "t=1" }.encode(),
            length: bytes.len() as u64,
        });
    }
    let value = encode_manifest(1, Some("n1"), &entries, -1, -1).unwrap();
    store
        .create_register(&at.manifest_key("").unwrap(), &value)
        .unwrap();
    let service = Arc::new(SnapshotService::new(
        Arc::clone(&blobs),
        "",
        "node-1",
        Arc::clone(&clock),
    ));
    let taker = Arc::new(
        SnapshotTakeService::new(
            Arc::clone(&blobs),
            "",
            DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap(),
            clock,
        )
        .with_stock_export(settings),
    );
    let router = routes(Router::new(), &service, Some(&taker)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, pool)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    let r = Rig {
        server,
        store,
        root,
        descriptors,
    };
    assert_eq!(send(&r, "PUT", "/_snapshot/stock", Some(repo)).status, 200);
    r
}

impl Rig {
    pub(crate) fn out(&self) -> PathBuf {
        self.root.join("out")
    }

    /// Every file under the target, as paths relative to it.
    fn written(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().display().to_string());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.out(), &self.out(), &mut out);
        out.sort();
        out
    }
}

pub(crate) fn send(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

pub(crate) const TAKE: &str = r#"{"indices":"logs"}"#;
pub(crate) const WAIT: &str = "?wait_for_completion=true";

#[test]
fn a_take_writes_a_repository_whose_blobs_hold_every_file_whole() {
    let r = rig(true, "take");
    let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let written = r.written();
    assert!(written.contains(&"index.latest".to_owned()), "{written:?}");
    assert!(written.contains(&"index-0".to_owned()), "{written:?}");
    let blobs: Vec<&String> = written
        .iter()
        .filter(|w| w.rsplit('/').next().is_some_and(|n| n.starts_with("__")))
        .collect();
    assert_eq!(
        blobs.len(),
        FILES.len().min(shard_files().len()),
        "{written:?}"
    );
    for (_, bytes) in shard_files() {
        let found = blobs
            .iter()
            .any(|b| std::fs::read(r.out().join(b)).is_ok_and(|got| got == bytes));
        assert!(found, "a file's bytes are in the repository");
    }
}

#[test]
fn a_second_take_keeps_the_first_and_reuses_the_index_id() {
    let r = rig(true, "second");
    for name in ["s1", "s2"] {
        let reply = send(
            &r,
            "PUT",
            &format!("/_snapshot/stock/{name}{WAIT}"),
            Some(TAKE),
        );
        assert_eq!(reply.status, 200, "{}", reply.text());
    }
    let written = r.written();
    assert!(written.contains(&"index-1".to_owned()), "{written:?}");
    let dirs: std::collections::BTreeSet<&str> = written
        .iter()
        .filter_map(|w| w.strip_prefix("indices/"))
        .filter_map(|w| w.split('/').next())
        .collect();
    assert_eq!(dirs.len(), 1, "one index id: {dirs:?}");
    let store = oss_blob::FsStore::new(r.out()).unwrap();
    let data = oss_meta::stock_load(&oss_meta::StockRepo::new(&store, "")).unwrap();
    for name in ["s1", "s2"] {
        let snap = data.snapshot(name).expect("both snapshots are listed");
        assert_eq!(data.indices_of(snap).count(), 1, "{name}");
    }
    let again = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(again.status, 400, "{}", again.text());
    assert!(again.text().contains("snapshot_already_exists"));
}

/// Both snapshots of a repository the Rust node wrote restore on the Rust
/// node, each into its own new index, every file whole.
#[test]
fn both_snapshots_of_a_repository_restore_on_the_rust_node() {
    use crate::snapshot_restore::{read, restored_uuid, send as send_restore};
    use crate::stock_restore::{POOL, register, rig as restore_rig};

    let r = rig(true, "restore-both");
    for name in ["s1", "s2"] {
        let reply = send(
            &r,
            "PUT",
            &format!("/_snapshot/stock/{name}{WAIT}"),
            Some(TAKE),
        );
        assert_eq!(reply.status, 200, "{}", reply.text());
    }
    let node = restore_rig(&r.root, true, POOL);
    register(&node, &r.out());
    for (snap, index) in [("s1", "logs_one"), ("s2", "logs_two")] {
        let body = format!(r#"{{"rename_pattern":"logs","rename_replacement":"{index}"}}"#);
        let reply = send_restore(
            &node,
            "POST",
            &format!("/_snapshot/stock/{snap}/_restore{WAIT}"),
            Some(&body),
        );
        assert_eq!(reply.status, 200, "{snap}: {}", reply.text());
        let uuid = restored_uuid(&node, index);
        let shard = ShardRef {
            index,
            uuid: &uuid,
            shard: 0,
        };
        let dir = format!("{}t=1/", shard.base_path("").unwrap());
        for (file, bytes) in shard_files() {
            assert_eq!(read(&node, &format!("{dir}{file}")), bytes, "{snap} {file}");
        }
    }
}

#[test]
fn with_the_switch_off_the_take_is_501_and_writes_nothing() {
    let r = rig(false, "off");
    let never = r.out().join("never");
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        never.display()
    );
    assert_eq!(send(&r, "PUT", "/_snapshot/idle", Some(&body)).status, 200);
    let reply = send(&r, "PUT", &format!("/_snapshot/idle/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(!never.exists(), "the target was not so much as opened");
    let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(r.written().is_empty());
}

#[test]
fn each_refusal_is_answered_before_anything_is_written() {
    let r = rig(true, "refuse");
    let cases: [(&str, &str, u16); 6] = [
        (
            "s1",
            r#"{"indices":"logs","include_global_state":true}"#,
            501,
        ),
        ("s1", r#"{"indices":"logs","partial":true}"#, 501),
        ("s1", r#"{"indices":"nope"}"#, 404),
        ("bad%20name", TAKE, 400),
        ("Sn%C3%A4p", TAKE, 400),
        ("s1", r#"{"indices":"logs,nope"}"#, 404),
    ];
    for (name, body, status) in cases {
        let reply = send(
            &r,
            "PUT",
            &format!("/_snapshot/stock/{name}{WAIT}"),
            Some(body),
        );
        assert_eq!(reply.status, status, "{name} {body}: {}", reply.text());
        assert!(r.written().is_empty(), "{name} {body}");
    }
}

/// A node whose request-body pool is `pool` bytes, exporting into `root/out`.
fn rig_pooled(pool: usize, tag: &str) -> Rig {
    let root = std::env::temp_dir().join(format!("oss-export-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("out")).unwrap();
    let root = root.canonicalize().unwrap();
    let settings = StockExportSettings {
        enabled: true,
        roots: SourceRoots::parse(root.to_str().unwrap(), ""),
        s3: None,
        chunk_bytes: 1 << 16,
    };
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        root.join("out").display()
    );
    stand_up(settings, root, &body, pool)
}

#[test]
fn a_pool_too_small_for_the_copy_is_429_and_writes_nothing() {
    // The take reserves two copy buffers and the largest `index-N` it reads.
    let held = (64 << 20) + 2 * (1 << 16);
    for (tag, pool) in [("pool-tiny", 8 << 20), ("pool-short", held - 1)] {
        let r = rig_pooled(pool, tag);
        let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
        assert_eq!(reply.status, 429, "{tag}: {}", reply.text());
        assert!(r.written().is_empty(), "{tag}: nothing was written");
    }
}

#[test]
fn a_target_outside_the_export_roots_is_refused_and_nothing_is_written() {
    let r = rig(true, "roots");
    let elsewhere =
        std::env::temp_dir().join(format!("oss-export-elsewhere-{}", std::process::id()));
    std::fs::create_dir_all(&elsewhere).unwrap();
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        elsewhere.display()
    );
    assert_eq!(send(&r, "PUT", "/_snapshot/loose", Some(&body)).status, 200);
    let reply = send(&r, "PUT", &format!("/_snapshot/loose/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

/// Registers an fs stock repository at `location` as `name`.
fn register_at(r: &Rig, name: &str, location: &str) {
    let body =
        format!(r#"{{"type":"fs","settings":{{"format":"stock","location":"{location}"}}}}"#);
    assert_eq!(
        send(r, "PUT", &format!("/_snapshot/{name}"), Some(&body)).status,
        200
    );
}

#[test]
fn a_location_reached_by_dot_dot_is_refused_and_nothing_is_written() {
    let r = rig(true, "dotdot");
    let sibling = r.root.join("sibling");
    std::fs::create_dir_all(&sibling).unwrap();
    register_at(&r, "up", &format!("{}/out/../sibling", r.root.display()));
    let reply = send(&r, "PUT", &format!("/_snapshot/up/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert_eq!(std::fs::read_dir(&sibling).unwrap().count(), 0);
    assert!(r.written().is_empty());
}

#[cfg(unix)]
#[test]
fn a_location_reached_through_a_symlink_is_refused_and_nothing_is_written() {
    let r = rig(true, "symlink");
    let outside = std::env::temp_dir().join(format!("oss-export-linked-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, r.root.join("link")).unwrap();
    register_at(&r, "linked", &format!("{}/link", r.root.display()));
    let reply = send(
        &r,
        "PUT",
        &format!("/_snapshot/linked/s1{WAIT}"),
        Some(TAKE),
    );
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

fn add_index(r: &Rig, name: &str, edit: impl FnOnce(&mut IndexRecord<'_>)) {
    let mut record = IndexRecord::new(name, "u2", 1);
    edit(&mut record);
    r.descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .unwrap();
}

fn refused(r: &Rig, repo: &str, name: &str, body: &str, status: u16) {
    let reply = send(
        r,
        "PUT",
        &format!("/_snapshot/{repo}/{name}{WAIT}"),
        Some(body),
    );
    assert_eq!(reply.status, status, "{name} {body}: {}", reply.text());
    assert!(r.written().is_empty(), "{name} {body}");
}

#[test]
fn a_name_is_held_to_128_bytes_and_the_name_characters() {
    let r = rig(true, "names");
    refused(&r, "stock", &"a".repeat(129), TAKE, 400);
    refused(&r, "stock", "a!", TAKE, 400);
    let reply = send(
        &r,
        "PUT",
        &format!("/_snapshot/stock/{}{WAIT}", "a".repeat(128)),
        Some(TAKE),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
}

#[test]
fn what_stock_cannot_hold_is_501_with_nothing_written() {
    let r = rig(true, "unholdable");
    add_index(&r, "packed", |rec| rec.bucket = Some("b1".into()));
    add_index(&r, "mapped", |rec| rec.mapping = Some("[1]".into()));
    add_index(&r, "listed", |rec| {
        rec.settings = br#"{"index.a":["x","y"]}"#
    });
    for index in ["packed", "mapped", "listed"] {
        refused(
            &r,
            "stock",
            "s1",
            &format!(r#"{{"indices":"{index}"}}"#),
            501,
        );
    }
}

#[test]
fn a_second_index_that_cannot_be_captured_fails_the_take_before_any_write() {
    let r = rig(true, "second-index");
    add_index(&r, "unpublished", |_| {});
    refused(&r, "stock", "s1", r#"{"indices":"logs,unpublished"}"#, 409);
}

#[test]
fn an_s3_repository_is_refused_read_only_and_without_an_endpoint() {
    let r = rig(true, "s3");
    let ro = r#"{"type":"s3","settings":{"format":"stock","bucket":"b","base_path":"p","readonly":true}}"#;
    assert_eq!(send(&r, "PUT", "/_snapshot/ro", Some(ro)).status, 200);
    refused(&r, "ro", "s1", TAKE, 400);
    let rw = r#"{"type":"s3","settings":{"format":"stock","bucket":"b","base_path":"p"}}"#;
    assert_eq!(send(&r, "PUT", "/_snapshot/rw", Some(rw)).status, 200);
    refused(&r, "rw", "s1", TAKE, 501);
}

/// Writes the recorded shard's export into `OSS_EXPORT_PROOF_DIR/repo` and
/// leaves it there, for a stock node to restore by hand.
#[test]
#[ignore = "run by hand with OSS_EXPORT_PROOF_DIR set, for the stock-node proof"]
fn the_export_is_left_in_a_directory_for_a_stock_node() {
    let dir = PathBuf::from(std::env::var("OSS_EXPORT_PROOF_DIR").expect("OSS_EXPORT_PROOF_DIR"));
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let dir = dir.canonicalize().unwrap();
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        dir.join("repo").display()
    );
    let settings = StockExportSettings {
        enabled: true,
        roots: SourceRoots::parse(dir.to_str().unwrap(), ""),
        s3: None,
        chunk_bytes: 1 << 16,
    };
    let r = rig_with(settings, dir, &body);
    let reply = send(
        &r,
        "PUT",
        &format!("/_snapshot/stock/snap-1{WAIT}"),
        Some(TAKE),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
}
