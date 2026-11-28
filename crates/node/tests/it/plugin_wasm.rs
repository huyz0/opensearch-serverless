//! A WASM module served as a plugin (docs/specs/plugin-system AC-1, AC-4,
//! AC-8): its descriptor is its manifest, its `rest` point answers a request
//! over HTTP, and a module that traps is answered and fenced like any plugin.

#![cfg(feature = "extensions")]

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use oss_ext::native::NativeInstance;
use oss_ext::rest::RestResponse;
use oss_ext_wasm::{Host, HostConfig, Unbounded};
use oss_mem::MemoryBudget;
use oss_node::lifecycle_bus::Bus;
use oss_node::plugins::{WasmPlugin, install};
use oss_rest::{Router, Server};

use crate::support;

const IN: u32 = 65_536;
const OUT: u32 = 131_072;
const CANNED: u32 = 8192;

fn describe(id: &str) -> String {
    format!(
        r#"{{"name":"{id}","version":"1.0.0","abi":"1.0","io_bytes":4096,"max_concurrent_calls":4,"points":[{{"id":"rest","version":"1.0.0"}}],"plugin":{{"api":"1.0","routes":[{{"method":"POST","path":"/_plugins/{id}/{{word}}","action":"echo"}}]}}}}"#
    )
}

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// A module that answers every `rest` call with `answer`, or runs `body`.
fn module(id: &str, answer: &[u8], body: Option<&str>) -> Vec<u8> {
    let d = describe(id);
    let call = body.map_or_else(
        || {
            format!(
                "(memory.copy (i32.const {OUT}) (i32.const {CANNED}) (i32.const {n})) \
                 (global.set $len (i32.const {n})) (i32.const 0)",
                n = answer.len()
            )
        },
        str::to_owned,
    );
    let text = format!(
        r#"(module
  (import "oss" "log" (func $log (param i32 i32)))
  (import "oss" "cancelled" (func $cancelled (result i32)))
  (memory (export "memory") 4)
  (data (i32.const 0) "{desc}")
  (data (i32.const {CANNED}) "{ans}")
  (global $len (mut i32) (i32.const 0))
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const {OUT}) (i32.const 0) (i32.const {dlen})) (i32.const {dlen}))
  (func (export "oss_input_region") (result i32) (i32.const {IN}))
  (func (export "oss_output_region") (result i32) (i32.const {OUT}))
  (func (export "oss_result_len") (result i32) (global.get $len))
  (func (export "oss_call") (param i32 i32 i32) (result i32) {call}))"#,
        desc = escaped(d.as_bytes()),
        ans = escaped(answer),
        dlen = d.len(),
    );
    wat::parse_str(text).expect("valid WAT")
}

fn served(id: &str, bytes: &[u8]) -> Server {
    let host = Host::new(HostConfig::default());
    let loaded = host.load(bytes, Arc::new(Unbounded)).expect("loads");
    let linked: HashMap<String, Arc<dyn NativeInstance>> =
        HashMap::from([(id.to_owned(), Arc::new(WasmPlugin::new(loaded)) as _)]);
    let backend: Arc<dyn oss_ext::host::Backend> = Arc::new(oss_ext::host::NoBackend);
    let policy = format!(r#"{{"plugins":[{{"id":"{id}","tier":"wasm"}}]}}"#);
    let router = install(
        Router::new(),
        &policy,
        &linked,
        &backend,
        || 0,
        &Bus::default(),
    )
    .expect("installs");
    let budget = MemoryBudget::new(&support::budget_config());
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    Server::start(listener, support::server_config(), router, &budget).expect("starts")
}

#[test]
fn a_wasm_plugin_serves_its_route_and_the_status_route_lists_it() {
    let answer = RestResponse {
        status: 201,
        headers: vec![("x-plugin".to_owned(), "wasm".to_owned())],
        body: b"hello".to_vec(),
    }
    .encode();
    let server = served("wasmecho", &module("wasmecho", &answer, None));
    let r = support::request(
        server.local_addr(),
        "POST",
        "/_plugins/wasmecho/oslo",
        Some(b"hi"),
    );
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.text(), "hello");
    assert_eq!(r.header("x-plugin"), Some("wasm"));
    let s = support::request(
        server.local_addr(),
        "GET",
        "/_plugins/_serverless/plugins",
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&s.text()).expect("json");
    assert_eq!(v["plugins"][0]["tier"], "wasm");
    assert_eq!(v["plugins"][0]["state"], "ready", "{v}");
    server.shutdown();
}

#[test]
fn a_module_that_traps_is_answered_and_the_next_call_is_served() {
    let answer = RestResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
    }
    .encode();
    // Traps on a request over 400 bytes; a small one is served.
    let body = format!(
        "(if (i32.gt_u (local.get 1) (i32.const 400)) (then unreachable)) \
         (memory.copy (i32.const {OUT}) (i32.const {CANNED}) (i32.const {n})) \
         (global.set $len (i32.const {n})) (i32.const 0)",
        n = answer.len()
    );
    let server = served("wasmtrap", &module("wasmtrap", &answer, Some(&body)));
    let big = vec![b'x'; 1000];
    let bad = support::request(
        server.local_addr(),
        "POST",
        "/_plugins/wasmtrap/a",
        Some(&big),
    );
    assert_eq!(bad.status, 400, "{}", bad.text());
    assert!(bad.text().contains("plugin_failed"), "{}", bad.text());
    let good = support::request(
        server.local_addr(),
        "POST",
        "/_plugins/wasmtrap/a",
        Some(b""),
    );
    assert_eq!(good.status, 200, "{}", good.text());
    server.shutdown();
}
