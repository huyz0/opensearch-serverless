//! Builds small WebAssembly modules that follow, or break, the extension ABI.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_ext_wasm::{Reservation, Reserver};

pub const IN: u32 = 65_536;
pub const OUT: u32 = 131_072;
pub const IO: u32 = 1024;

/// The descriptor text of a module named `hostile` with one point.
pub fn descriptor(extra: &str) -> String {
    format!(
        r#"{{"name":"hostile","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8{extra},"points":[{{"id":"echo","version":"1.0.0"}}]}}"#
    )
}

fn escaped(text: &str) -> String {
    text.bytes().map(|b| format!("\\{b:02x}")).collect()
}

/// The pieces of a module; `Default` is a module that echoes its input.
pub struct Parts<'a> {
    pub imports: &'a str,
    pub memory: &'a str,
    pub descriptor: String,
    pub abi: u32,
    pub body: &'a str,
    pub extra: &'a str,
}

impl Default for Parts<'_> {
    fn default() -> Self {
        Self {
            imports: "",
            memory: "4",
            descriptor: descriptor(""),
            abi: 65_536,
            body: ECHO,
            extra: "",
        }
    }
}

pub const ECHO: &str = "(memory.copy (i32.const 131072) (i32.const 65536) (local.get 1)) \
    (global.set $len (local.get 1)) (i32.const 0)";

/// The module for `p`, as binary.
pub fn build(p: &Parts<'_>) -> Vec<u8> {
    let d = &p.descriptor;
    let text = format!(
        r#"(module
  {imports}
  (import "oss" "log" (func $log (param i32 i32)))
  (import "oss" "cancelled" (func $cancelled (result i32)))
  (memory (export "memory") {memory})
  (data (i32.const 0) "{desc}")
  (data (i32.const 400) "{detail}")
  (global $len (mut i32) (i32.const 0))
  (func (export "oss_abi_version") (result i32) (i32.const {abi}))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const {OUT}) (i32.const 0) (i32.const {dlen})) (i32.const {dlen}))
  (func (export "oss_input_region") (result i32) (i32.const {IN}))
  (func (export "oss_output_region") (result i32) (i32.const {OUT}))
  (func (export "oss_result_len") (result i32) (global.get $len))
  (func (export "oss_call") (param i32 i32 i32) (result i32) {body})
  {extra})"#,
        imports = p.imports,
        memory = p.memory,
        desc = escaped(d),
        detail = escaped("bad\nnews"),
        abi = p.abi as i32,
        dlen = d.len(),
        body = p.body,
        extra = p.extra,
    );
    wat::parse_str(text).expect("the test module is valid WAT")
}

pub fn with_body(body: &str) -> Vec<u8> {
    build(&Parts {
        body,
        ..Parts::default()
    })
}

/// A bounded pool that counts what is held and the most it ever held.
#[derive(Default)]
pub struct Pool {
    pub budget: AtomicU64,
    pub held: AtomicU64,
    pub peak: AtomicU64,
}

impl Pool {
    pub fn of(budget: u64) -> Arc<Self> {
        let p = Self::default();
        p.budget.store(budget, Ordering::SeqCst);
        Arc::new(p)
    }
}

struct Release(Arc<Pool>, u64);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.held.fetch_sub(self.1, Ordering::SeqCst);
    }
}

/// A pool handle that can grant: `Reserver` is implemented for it.
pub struct Handle(pub Arc<Pool>);

impl Reserver for Handle {
    fn reserve(&self, bytes: u64) -> Option<Reservation> {
        let p = &self.0;
        let now = p
            .held
            .fetch_add(bytes, Ordering::SeqCst)
            .saturating_add(bytes);
        if now > p.budget.load(Ordering::SeqCst) {
            p.held.fetch_sub(bytes, Ordering::SeqCst);
            return None;
        }
        p.peak.fetch_max(now, Ordering::SeqCst);
        Some(Box::new(Release(Arc::clone(p), bytes)))
    }
}
