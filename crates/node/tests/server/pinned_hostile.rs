//! docs/specs/extension-analyzer AC-16: hostile modules pinned by an index
//! fail the searches that call them with a typed refusal, and leave the node
//! serving and its extensions pool holding only the loaded code.
#![cfg(feature = "extensions")]

use std::sync::Arc;

use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore};
use oss_ext_wasm::{Host, HostConfig};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::extension_modules::ModuleSource;
use oss_testkit::TempDir;

use super::pinned_read::{module, rig_with};
use crate::support::request;

const IO: u32 = 4 * 1024 * 1024;

/// An analyzer module whose call runs `body` and whose answer is `len` bytes long.
fn hostile(body: &str, len: &str) -> Vec<u8> {
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
  (func (export "oss_result_len") (result i32) ({len}))
  (func (export "oss_call") (param i32 i32 i32) (result i32) {body}))"#,
        dl = d.len(),
    ))
    .expect("valid module")
}

/// What the hostile modules do, each with the way it is hostile.
fn kinds() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "spin",
            hostile("(loop $l (br $l)) (i32.const 0)", "i32.const 0"),
        ),
        ("trap", hostile("unreachable", "i32.const 0")),
        (
            "memory growth",
            hostile(
                "(drop (memory.grow (i32.const 60000))) (i32.const 0)",
                "i32.const 0",
            ),
        ),
        ("undefined status", hostile("(i32.const 99)", "i32.const 0")),
        (
            "oversized length",
            hostile("(i32.const 0)", "i32.const 2147483647"),
        ),
        ("empty answer", hostile("(i32.const 0)", "i32.const 0")),
    ]
}

// AC-16 (through a node's search path)
#[test]
fn a_hostile_module_fails_each_search_typed_and_the_node_keeps_serving() {
    let mut failures = Vec::new();
    for (name, wasm) in kinds() {
        let root = TempDir::new("pinned-hostile");
        let dir = ByteBuffersDirectory::new();
        let r = rig_with(root.path(), &dir, &wasm, (50_000_000, 400));
        let budget = MemoryBudget::new(
            &BudgetConfig::new(256 << 20)
                .with_pool(Pool::RequestBodies, 32 << 20)
                .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
                .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
                .expect("fits"),
        );
        let source = Arc::new(ModuleSource::new(
            Arc::new(FsStore::new(root.path()).expect("store")) as Arc<dyn BlobStore>,
            "",
            Host::new(HostConfig::default()),
            budget.clone(),
            16,
        ));
        assert!(r.service.set_modules(source));
        let addr = r.server.local_addr();
        let body: &'static [u8] = br#"{"query":{"match":{"t":"zzzzzzz"}}}"#;
        let callers: Vec<_> = (0..6)
            .map(|_| std::thread::spawn(move || request(addr, "POST", "/logs/_search", Some(body))))
            .collect();
        for c in callers {
            let reply = c.join().expect("joined");
            let typed = reply.text().contains("extension_");
            if reply.status == 200 || reply.status == 500 || !typed {
                failures.push(format!("{name}: {} {}", reply.status, reply.text()));
            }
        }
        // The pool holds the module's code and nothing a call took.
        let held = budget.in_use(Pool::Extensions);
        assert!(held < 64 << 20, "{name}: {held} bytes held after the calls");
        drop(r);
    }
    assert!(failures.is_empty(), "untyped or answered: {failures:#?}");

    // A well-behaved module on the same kind of node still answers.
    let root = TempDir::new("pinned-hostile-good");
    let dir = ByteBuffersDirectory::new();
    let r = rig_with(root.path(), &dir, &module(), (10_000_000, 10_000));
    assert!(
        r.service
            .set_modules(super::pinned_read::modules(root.path()))
    );
    let reply = super::pinned_read::matched(&r);
    assert_eq!(reply.status, 200, "{}", reply.text());
}
