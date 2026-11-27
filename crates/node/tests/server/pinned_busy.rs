//! docs/specs/extension-analyzer AC-18: a reader whose extensions pool cannot
//! hold the module an index pins answers 429 `extension_busy`, not a
//! fallback to the built-in analyzer, and the next search opens once the
//! room is back.
#![cfg(feature = "extensions")]

use std::sync::Arc;

use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore};
use oss_ext_wasm::{Host, HostConfig};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::extension_modules::ModuleSource;
use oss_testkit::TempDir;

use oss_ext::pin::{Pin, ResolvedLimits};

use super::pinned_read::{matched, module, modules, rig, rig_with};
use crate::support::request;

// AC-18 (429 through a running search, a real full pool)
#[test]
fn a_search_whose_pool_cannot_hold_the_module_is_a_429_and_the_next_one_opens() {
    let root = TempDir::new("pinned-full-pool");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let source = ModuleSource::new(
        Arc::new(FsStore::new(root.path()).expect("store")) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        budget.clone(),
        16,
    );
    assert!(r.service.set_modules(Arc::new(source)));
    // Every byte but one MiB of the pool is held by something else.
    let taken = budget
        .try_reserve(Pool::Extensions, budget.limit(Pool::Extensions) - (1 << 20))
        .expect("reserved");
    let body = br#"{"query":{"match":{"t":"zzzzzzz"}}}"#;
    let busy = request(r.server.local_addr(), "POST", "/logs/_search", Some(body));
    assert_eq!(busy.status, 429, "{}", busy.text());
    assert!(busy.text().contains("extension_busy"), "{}", busy.text());
    assert!(!busy.text().contains(r#""hits""#), "{}", busy.text());
    drop(taken);
    let again = request(r.server.local_addr(), "POST", "/logs/_search", Some(body));
    assert_eq!(again.status, 200, "{}", again.text());
    assert!(again.text().contains(r#""value":1"#), "{}", again.text());
}

/// An opener with no room to compile until `room` is set: the refusal a full extensions pool
/// gives, which carries the words of a capacity refusal; then the real one.
struct NoRoomYet {
    room: Arc<std::sync::atomic::AtomicBool>,
    then: Arc<ModuleSource>,
}

impl oss_node::search_modules::ModuleOpener for NoRoomYet {
    fn open(
        &self,
        index: &str,
        pins: Vec<(Pin, ResolvedLimits)>,
    ) -> Result<Arc<dyn oss_reader::pinned::PinnedTokens>, String> {
        if self.room.load(std::sync::atomic::Ordering::SeqCst) {
            return oss_node::search_modules::ModuleOpener::open(self.then.as_ref(), index, pins);
        }
        Err(format!(
            "extension_unavailable: module [m]: {} (the pool is full)",
            oss_node::search_modules::NO_CAPACITY
        ))
    }
}

// AC-14 (429 through a running search)
#[test]
fn a_search_that_finds_no_room_to_compile_is_a_429_and_the_next_one_opens() {
    let root = TempDir::new("pinned-busy");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    let room = Arc::new(std::sync::atomic::AtomicBool::new(false));
    assert!(r.service.set_modules(Arc::new(NoRoomYet {
        room: Arc::clone(&room),
        then: modules(root.path()),
    })));
    let busy = matched(&r);
    assert_eq!(busy.status, 429, "{}", busy.text());
    assert!(busy.text().contains("extension_busy"), "{}", busy.text());
    room.store(true, std::sync::atomic::Ordering::SeqCst);
    let again = matched(&r);
    assert_eq!(again.status, 200, "{}", again.text());
    assert!(again.text().contains(r#""value":1"#), "{}", again.text());
}

/// An analyzer module whose every call spins until the host stops it.
fn spinning() -> Vec<u8> {
    const IO: u32 = 4 * 1024 * 1024;
    let d = format!(
        r#"{{"name":"ana","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8,"points":[{{"id":"analyzer","version":"1.0.0"}}]}}"#
    );
    let escaped: String = d.bytes().map(|b| format!("\\{b:02x}")).collect();
    let out_at = 65_536 + IO;
    wat::parse_str(format!(
        r#"(module
  (memory (export "memory") 130)
  (data (i32.const 0) "{escaped}")
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const {out_at}) (i32.const 0) (i32.const {dl})) (i32.const {dl}))
  (func (export "oss_input_region") (result i32) (i32.const 65536))
  (func (export "oss_output_region") (result i32) (i32.const {out_at}))
  (func (export "oss_result_len") (result i32) (i32.const 0))
  (func (export "oss_call") (param i32 i32 i32) (result i32)
    (loop $l (br $l)) (i32.const 0)))"#,
        dl = d.len(),
    ))
    .expect("valid module")
}

// AC-18 (a tenant's third concurrent call)
#[test]
fn a_tenants_third_concurrent_call_is_a_429() {
    let root = TempDir::new("pinned-third");
    let dir = ByteBuffersDirectory::new();
    // Fuel for far longer than the 3 s deadline, so a call ends at the deadline.
    let r = rig_with(root.path(), &dir, &spinning(), (u64::MAX / 2, 3_000));
    assert!(r.service.set_modules(modules(root.path())));
    let addr = r.server.local_addr();
    let body: &'static [u8] = br#"{"query":{"match":{"t":"zzzzzzz"}}}"#;
    let spinners: Vec<_> = (0..2)
        .map(|_| {
            std::thread::spawn(move || request(addr, "POST", "/logs/_search", Some(body)).status)
        })
        .collect();
    std::thread::sleep(std::time::Duration::from_millis(800));
    let third = matched(&r);
    assert_eq!(third.status, 429, "{}", third.text());
    assert!(third.text().contains("extension_busy"), "{}", third.text());
    for s in spinners {
        let status = s.join().expect("joined");
        assert_ne!(
            status, 429,
            "the two calls held the share and were not refused"
        );
        assert_ne!(status, 200, "a spinning module never answers");
    }
}
