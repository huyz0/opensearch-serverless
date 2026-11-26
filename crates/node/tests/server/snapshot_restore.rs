//! docs/specs/snapshot-restore: a shallow snapshot restored over REST into
//! new indices, as the shell restores one, at the request cost the budget
//! table holds.

// The fixtures the Java reads are a few hundred bytes.
#![allow(clippy::disallowed_methods)]

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator, S3Op, SliceSource};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::index_delete::{DeleteService, NoSnapshots};
use oss_node::restore_background::{BackgroundRestores, Settings};
use oss_node::snapshot_restore_route::{SnapshotRestoreService, routes as restore_routes};
use oss_node::snapshot_route::{SnapshotService, routes};
use oss_node::snapshot_take_route::SnapshotTakeService;
use oss_node::stock_restore_route::StockRestoreSettings;
use oss_rest::{Router, Server};
use oss_testkit::budget;

use crate::snapshot_take::TAKE;

pub(crate) struct Rig {
    pub(crate) server: Server,
    pub(crate) store: Arc<Emulator>,
    pub(crate) descriptors: DescriptorStore,
    pub(crate) budget: MemoryBudget,
    pub(crate) restores: Arc<BackgroundRestores>,
    pub(crate) deletes: Arc<DeleteService>,
}

pub(crate) fn rig(restore: bool) -> Rig {
    rig_with(restore, "")
}

pub(crate) fn rig_with(restore: bool, system: &str) -> Rig {
    rig_sized(restore, system, 64 << 20, 64)
}

/// A rig whose request-body pool holds `pool` bytes and whose restore moves
/// each file through two buffers of `chunk` bytes.
pub(crate) fn rig_sized(restore: bool, system: &str, pool: usize, chunk: usize) -> Rig {
    rig_stock(restore, system, pool, chunk, StockRestoreSettings::off())
}

/// A rig that also restores from stock repositories as `stock` says.
pub(crate) fn rig_stock(
    restore: bool,
    system: &str,
    pool: usize,
    chunk: usize,
    stock: StockRestoreSettings,
) -> Rig {
    rig_full(restore, system, pool, chunk, stock, Settings::off())
}

/// A rig whose restores run in the background as `background` says.
pub(crate) fn rig_full(
    restore: bool,
    system: &str,
    pool: usize,
    chunk: usize,
    stock: StockRestoreSettings,
    background: Settings,
) -> Rig {
    rig_aliased(restore, system, pool, chunk, stock, background, false)
}

/// A rig whose take records aliases when `capture_aliases`.
pub(crate) fn rig_aliased(
    restore: bool,
    system: &str,
    pool: usize,
    chunk: usize,
    stock: StockRestoreSettings,
    background: Settings,
    capture_aliases: bool,
) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
    let service = Arc::new(SnapshotService::new(
        Arc::clone(&blobs),
        "",
        "node-1",
        Arc::clone(&clock),
    ));
    let descriptors =
        || DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
    let taker = Arc::new(
        SnapshotTakeService::new(Arc::clone(&blobs), "", descriptors(), Arc::clone(&clock))
            .with_capture_aliases(capture_aliases)
            .with_capture_global_state(true)
            .with_partial_shards(true),
    );
    let deletes = Arc::new(
        DeleteService::new(
            &blobs,
            "",
            DeleteMode::Conditional,
            Arc::clone(&clock),
            Arc::new(NoSnapshots),
            1 << 16,
            8,
        )
        .unwrap(),
    );
    let removes = Arc::clone(&deletes);
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, pool)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let restores = BackgroundRestores::new(background, budget.clone());
    let restorer = Arc::new(
        SnapshotRestoreService::new(
            Arc::clone(&blobs),
            "",
            descriptors(),
            deletes,
            clock,
            restore,
            chunk,
        )
        .with_system_indices(Arc::new(
            oss_coordinator::system_indices::SystemIndices::parse(system),
        ))
        .with_background(Arc::clone(&restores))
        .with_stock(stock),
    );
    let router = routes(Router::new(), &service, Some(&taker)).expect("routes");
    let router = restore_routes(router, restorer).expect("restore routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        descriptors: descriptors(),
        budget,
        restores,
        deletes: removes,
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

pub(crate) fn requests(r: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum()
}

/// An index of `shards` shards, each with one file `_0.si` of `bytes`.
pub(crate) fn live_index(r: &Rig, name: &str, uuid: &str, shards: u32, bytes: &[u8]) {
    r.descriptors
        .create(
            &IndexRecord::new(name, uuid, shards),
            1,
            &mut vec![0u8; 1 << 16],
        )
        .unwrap();
    for shard in 0..shards {
        let at = ShardRef {
            index: name,
            uuid,
            shard,
        };
        let key = format!("{}t=1/_0.si", at.base_path("").unwrap());
        r.store
            .put(&key, &mut SliceSource::new(bytes), &mut [0u8; 64])
            .unwrap();
        let entries = [ManifestEntry {
            name: "_0.si",
            location: FileLocation::Whole { term_dir: "t=1" }.encode(),
            length: bytes.len() as u64,
        }];
        let value = encode_manifest(1, Some("n1"), &entries, -1, -1).unwrap();
        r.store
            .create_register(&at.manifest_key("").unwrap(), &value)
            .unwrap();
    }
}

pub(crate) fn read(r: &Rig, key: &str) -> Vec<u8> {
    let mut out = Vec::new();
    r.store
        .get(key, ByteRange::Full, &mut [0u8; 64], &mut |c: &[u8]| {
            out.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
    out
}

/// A rig holding a shallow snapshot `daily` of `logs`, whose two shards
/// each hold one file of seven bytes.
pub(crate) fn taken(restore: bool) -> Rig {
    seed(rig(restore))
}

/// `r` with the repository registered and the snapshot `daily` taken.
pub(crate) fn seed(r: Rig) -> Rig {
    let body =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some(body)).status,
        200
    );
    live_index(&r, "logs", "u1", 2, b"payload");
    let made = send(
        &r,
        "PUT",
        "/_snapshot/backups/daily?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    r
}

/// A rig whose take records aliases when `capture`, with `setup` run on the
/// live index before the snapshot is taken.
pub(crate) fn taken_with_aliases(restore: bool, capture: bool, setup: fn(&Rig)) -> Rig {
    let r = rig_aliased(
        restore,
        "",
        64 << 20,
        64,
        StockRestoreSettings::off(),
        Settings::off(),
        capture,
    );
    seed_with(&r, setup);
    r
}

/// Registers the repository, makes `logs`, runs `setup` and takes `daily`.
pub(crate) fn seed_with(r: &Rig, setup: fn(&Rig)) {
    let body =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(send(r, "PUT", "/_snapshot/backups", Some(body)).status, 200);
    live_index(r, "logs", "u1", 2, b"payload");
    setup(r);
    let made = send(
        r,
        "PUT",
        "/_snapshot/backups/daily?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(made.status, 200, "{}", made.text());
}

pub(crate) const RESTORE: &str = "/_snapshot/backups/daily/_restore?wait_for_completion=true";

pub(crate) fn restored_uuid(r: &Rig, name: &str) -> String {
    let mut buf = vec![0u8; 1 << 16];
    let found = r
        .descriptors
        .get(name, &mut buf)
        .unwrap()
        .expect("descriptor");
    found.uuid.to_string()
}

#[test]
fn a_restore_creates_the_index_and_copies_every_file() {
    let r = taken(true);
    let reply = send(
        &r,
        "POST",
        RESTORE,
        Some(r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let uuid = restored_uuid(&r, "copy");
    for shard in 0..2 {
        let at = ShardRef {
            index: "copy",
            uuid: &uuid,
            shard,
        };
        let blob = format!("{}t=1/_0.si", at.base_path("").unwrap());
        assert_eq!(read(&r, &blob), b"payload");
        let manifest = read_register(&r, &at.manifest_key("").unwrap());
        assert!(!manifest.is_empty(), "the manifest landed");
    }
    // The source is untouched.
    assert!(
        r.descriptors
            .get("logs", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_some()
    );
}

pub(crate) fn read_register(r: &Rig, key: &str) -> Vec<u8> {
    let mut buf = vec![0u8; 1 << 16];
    let found = r
        .store
        .read_register(key, &mut buf)
        .unwrap()
        .expect("register");
    buf[..found.len].to_vec()
}

#[test]
fn a_restore_with_the_switch_off_answers_501_and_asks_the_store_nothing() {
    let r = taken(false);
    let before = requests(&r);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 501);
    assert_eq!(requests(&r), before);
}

#[test]
fn a_target_that_exists_is_refused_before_anything_is_written() {
    let r = taken(true);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 400, "restoring over the source's own name");
    assert!(
        reply.text().contains("index_already_exists"),
        "{}",
        reply.text()
    );
}

#[test]
fn a_snapshot_that_does_not_exist_is_404() {
    let r = taken(true);
    let reply = send(&r, "POST", "/_snapshot/backups/nope/_restore", None);
    assert_eq!(reply.status, 404);
}

#[test]
fn an_index_the_snapshot_did_not_capture_is_404() {
    let r = taken(true);
    let reply = send(&r, "POST", RESTORE, Some(r#"{"indices":"other"}"#));
    assert_eq!(reply.status, 404);
}

#[test]
fn a_restore_that_fails_part_way_removes_what_it_created() {
    let r = taken(true);
    // A file the snapshot names is gone: the copy fails on its first read.
    let key = {
        let at = ShardRef {
            index: "logs",
            uuid: "u1",
            shard: 1,
        };
        format!("{}t=1/_0.si", at.base_path("").unwrap())
    };
    r.store.delete(&key).unwrap();
    let before = requests(&r);
    let reply = send(
        &r,
        "POST",
        RESTORE,
        Some(r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#),
    );
    assert!(reply.status >= 400, "{}", reply.status);
    assert!(
        r.descriptors
            .get("copy", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_none(),
        "the failed restore's index is gone"
    );
    budget::check(
        "snapshot.restore_cleanup.requests",
        requests(&r) - before,
        "a restore that fails on its second shard",
    );
}

#[test]
fn a_restore_costs_what_the_budget_says() {
    let r = taken(true);
    let before = requests(&r);
    let reply = send(
        &r,
        "POST",
        RESTORE,
        Some(r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#),
    );
    assert_eq!(reply.status, 200);
    budget::check(
        "snapshot.restore_two.requests",
        requests(&r) - before,
        "a restore of one index of two shards",
    );
}

#[test]
fn the_copy_buffers_are_four_mebibytes() {
    assert_eq!(oss_node::snapshot_restore_route::COPY_CHUNK_BYTES, 4 << 20);
}

#[test]
fn a_snapshot_that_copied_its_data_is_refused_501() {
    let r = rig(true);
    live_index(&r, "logs", "u1", 1, b"payload");
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let taker = oss_meta::SnapshotTaker::new(Arc::clone(&blobs), "");
    let described = taker
        .describe(&r.descriptors, "logs", &mut vec![0u8; 1 << 16])
        .expect("described");
    let draft = oss_format::SnapshotDraft {
        repo: "backups".into(),
        name: "daily".into(),
        uuid: "s".into(),
        storage_generation: Some("genAAAAAAAAAAAAAAAAAAA".into()),
        shallow: false,
        start_millis: 1,
        end_millis: 2,
        global_state: None,
        indices: vec![described.capture(Vec::new(), false)],
    };
    let mut value = vec![0u8; 1 << 16];
    let held = taker.reserve(&draft, &mut value).expect("reserved");
    taker
        .publish_marker(&held, &draft, &mut value)
        .expect("marker");
    taker.finish(&held, &draft, &mut value).expect("finished");
    let reply = send(&r, "POST", "/_snapshot/backups/daily/_restore", None);
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(reply.text().contains("copied its data"), "{}", reply.text());
}

#[test]
fn a_store_that_fails_reading_the_snapshot_answers_503() {
    let r = taken(true);
    r.store.fail_next(S3Op::Get, 1);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 503, "{}", reply.text());
}

#[test]
fn a_restore_not_asked_to_wait_answers_accepted_and_one_asked_answers_the_indices() {
    let r = taken(true);
    let rename = r#"{"rename_pattern":"logs","rename_replacement":"a"}"#;
    let quick = send(
        &r,
        "POST",
        "/_snapshot/backups/daily/_restore",
        Some(rename),
    );
    assert_eq!(quick.text(), r#"{"accepted":true}"#);
    let rename = r#"{"rename_pattern":"logs","rename_replacement":"b"}"#;
    let full = send(&r, "POST", RESTORE, Some(rename));
    assert!(
        full.text().contains(r#""indices":["b"]"#),
        "{}",
        full.text()
    );
}
