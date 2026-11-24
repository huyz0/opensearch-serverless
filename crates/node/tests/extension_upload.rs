//! docs/specs/extension-abi AC-7: the module upload route and the one read a
//! node makes of a module.
#![cfg(feature = "extensions")]

#[path = "support/mod.rs"]
mod support;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_ext_wasm::{Host, HostConfig};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::extension_route::{self, ExtensionService};
use oss_node::extension_store::{self, StoreError};
use oss_rest::{Router, Server};
use oss_testkit::budget;

const DESCRIPTOR: &str = r#"{"name":"upload_test","version":"1.2.3","abi":"1.0","io_bytes":1024,"max_concurrent_calls":4,"points":[{"id":"echo","version":"1.0.0"}]}"#;

/// A module that follows the ABI and does nothing.
fn module() -> Vec<u8> {
    let escaped: String = DESCRIPTOR.bytes().map(|b| format!("\\{b:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
  (import "oss" "log" (func (param i32 i32)))
  (memory (export "memory") 4)
  (data (i32.const 0) "{escaped}")
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const 131072) (i32.const 0) (i32.const {len})) (i32.const {len}))
  (func (export "oss_input_region") (result i32) (i32.const 65536))
  (func (export "oss_output_region") (result i32) (i32.const 131072))
  (func (export "oss_result_len") (result i32) (i32.const 0))
  (func (export "oss_call") (param i32 i32 i32) (result i32) (i32.const 0)))"#,
        len = DESCRIPTOR.len()
    ))
    .expect("valid module")
}

struct Rig {
    server: Server,
    store: Arc<dyn BlobStore>,
    budget: MemoryBudget,
}

fn rig() -> Rig {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let service = ExtensionService::new(
        Arc::clone(&store),
        "",
        Host::new(HostConfig::default()),
        budget.clone(),
    );
    let router = extension_route::routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &budget,
    )
    .expect("starts");
    Rig {
        server,
        store,
        budget,
    }
}

fn put(r: &Rig, body: &[u8]) -> support::Reply {
    support::request(
        r.server.local_addr(),
        "PUT",
        "/_serverless/extensions/modules",
        Some(body),
    )
}

fn count(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

// docs/specs/extension-abi AC-7
#[test]
fn a_module_is_one_put_under_its_hash_and_a_repeat_adds_none() {
    let r = rig();
    let wasm = module();
    let hash = extension_store::hash_of(&wasm);
    let before = r.store.meter().total();
    let first = put(&r, &wasm);
    budget::check(
        "extension.upload.requests",
        r.store.meter().total() - before,
        "a HEAD and a PUT",
    );
    assert_eq!(first.status, 201, "{}", first.text());
    assert_eq!(
        first.text(),
        format!(r#"{{"id":"{hash}","name":"upload_test","version":"1.2.3","created":true}}"#)
    );
    assert_eq!(count(&r, S3Op::Put), 1);
    let (puts, before) = (count(&r, S3Op::Put), r.store.meter().total());
    let again = put(&r, &wasm);
    budget::check(
        "extension.upload_repeat.requests",
        r.store.meter().total() - before,
        "a HEAD",
    );
    assert_eq!(again.status, 200, "{}", again.text());
    assert!(again.text().ends_with(r#""created":false}"#));
    assert_eq!(count(&r, S3Op::Put), puts, "a repeat adds no PUT");
    assert_eq!(
        r.budget.in_use(Pool::Extensions),
        0,
        "the compile is released"
    );
}

#[test]
fn a_node_reads_a_module_with_one_get_and_a_call_adds_none() {
    let r = rig();
    let wasm = module();
    let hash = extension_store::hash_of(&wasm);
    assert_eq!(put(&r, &wasm).status, 201);
    let host = Host::new(HostConfig::default());
    let (gets, before) = (count(&r, S3Op::Get), r.store.meter().total());
    let m = extension_store::load(r.store.as_ref(), "", &hash, &host, &r.budget).expect("loads");
    assert_eq!(count(&r, S3Op::Get), gets + 1, "the one request is a GET");
    budget::check(
        "extension.load.requests",
        r.store.meter().total() - before,
        "one GET",
    );
    let before = r.store.meter().total();
    for _ in 0..5 {
        m.call("t", 0, b"x").expect("calls");
    }
    budget::check(
        "extension.call.requests",
        r.store.meter().total() - before,
        "five calls",
    );
}

#[test]
fn bytes_that_do_not_hash_to_their_key_are_refused() {
    let r = rig();
    let wasm = module();
    let hash = extension_store::hash_of(&wasm);
    let mut other = module();
    other.push(0);
    let mut buf = vec![0_u8; 4096];
    r.store
        .put(
            &extension_store::key("", &hash),
            &mut oss_blob::SliceSource::new(&other),
            &mut buf,
        )
        .expect("planted");
    let host = Host::new(HostConfig::default());
    let e = extension_store::load(r.store.as_ref(), "", &hash, &host, &r.budget).err();
    assert!(matches!(e, Some(StoreError::Mismatch)), "{e:?}");
}

#[test]
fn a_module_that_is_refused_is_not_stored() {
    let r = rig();
    let junk = put(&r, b"not a module");
    assert_eq!(junk.status, 400, "{}", junk.text());
    assert!(
        junk.text().contains("extension_rejected"),
        "{}",
        junk.text()
    );
    let big = put(&r, &vec![0_u8; 8 * 1024 * 1024 + 1]);
    assert_eq!(big.status, 413, "{}", big.text());
    assert_eq!(count(&r, S3Op::Put), 0);
}

#[test]
fn a_second_upload_while_one_is_in_flight_answers_429() {
    let r = rig();
    // The first upload sends its head and half its body, then waits.
    let mut slow = std::net::TcpStream::connect(r.server.local_addr()).expect("connects");
    slow.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    let wasm = module();
    let head = format!(
        "PUT /_serverless/extensions/modules HTTP/1.1\r\nhost: t\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        wasm.len()
    );
    slow.write_all(head.as_bytes()).expect("head");
    slow.write_all(&wasm[..8]).expect("part");
    std::thread::sleep(Duration::from_millis(300));
    let second = put(&r, &wasm);
    assert_eq!(second.status, 429, "{}", second.text());
    assert_eq!(count(&r, S3Op::Put), 0);
    slow.write_all(&wasm[8..]).expect("rest");
    let mut reply = String::new();
    let mut buf = [0_u8; 512];
    while !reply.contains("\r\n\r\n") || !reply.contains("created") {
        let n = slow.read(&mut buf).expect("reads");
        assert!(n > 0, "closed early: {reply}");
        reply.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(reply.starts_with("HTTP/1.1 201"), "{reply}");
    assert_eq!(put(&r, &wasm).status, 200, "the place is free again");
}
