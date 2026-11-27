//! docs/specs/extension-analyzer AC-13, AC-14: a reader opening an index
//! that pins an analyzer loads the module and parses its query text with
//! it, and a reader that cannot load it refuses rather than answer with the
//! standard analyzer.
#![cfg(feature = "extensions")]

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore, PooledStore};
use oss_ext::CallError;
use oss_ext::analyzer::{Analyze, Token};
use oss_ext::pin::{Pin, ResolvedLimits, render};
use oss_ext_wasm::{Host, HostConfig};
use oss_format::ShardRef;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_node::extension_modules::ModuleSource;
use oss_node::extension_store;
use oss_node::search::start_reader;
use oss_testkit::TempDir;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};
use oss_writer::publish::ShardPublisher;

use support::{request, server_config};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};
const IO: u32 = 4 * 1024 * 1024;
const MAPPING: &str = r#"{"properties":{"t":{"type":"text","analyzer":"ext_a"}}}"#;

/// Every text is the one term `probe`.
struct Probe;

impl Analyze for Probe {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        Ok(vec![Token {
            term: "probe".to_owned(),
            start: 0,
            end: u32::try_from(text.encode_utf16().count()).expect("small"),
            increment: 1,
        }])
    }
}

fn word(out: &mut Vec<u8>, w: u32) {
    out.extend_from_slice(&w.to_le_bytes());
}

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// An analyzer module whose every call answers the one token `probe`.
pub(crate) fn module() -> Vec<u8> {
    let mut o = Vec::new();
    for w in [1, 5] {
        word(&mut o, w);
    }
    o.extend_from_slice(b"probe");
    for w in [0, 5, 1] {
        word(&mut o, w);
    }
    let d = format!(
        r#"{{"name":"ana","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8,"points":[{{"id":"analyzer","version":"1.0.0"}}]}}"#
    );
    let n = o.len();
    let out_at = 65_536 + IO;
    wat::parse_str(format!(
        r#"(module
  (memory (export "memory") 130)
  (data (i32.const 0) "{d}")
  (data (i32.const 1000) "{o}")
  (global $len (mut i32) (i32.const 0))
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const {out_at}) (i32.const 0) (i32.const {dl})) (i32.const {dl}))
  (func (export "oss_input_region") (result i32) (i32.const 65536))
  (func (export "oss_output_region") (result i32) (i32.const {out_at}))
  (func (export "oss_result_len") (result i32) (global.get $len))
  (func (export "oss_call") (param i32 i32 i32) (result i32)
    (memory.copy (i32.const {out_at}) (i32.const 1000) (i32.const {n}))
    (global.set $len (i32.const {n})) (i32.const 0)))"#,
        d = escaped(d.as_bytes()),
        o = escaped(&o),
        dl = d.len(),
    ))
    .expect("valid module")
}

pub(crate) fn descriptor(hash: &str, fuel: u64, deadline_ms: u64) -> String {
    let pin = Pin {
        name: "ext_a".into(),
        module: hash.into(),
        version: 1,
    };
    let limits = ResolvedLimits {
        fuel,
        memory_bytes: 16 << 20,
        deadline_ms,
    };
    let settings: Vec<String> = render(&[(pin, limits)])
        .iter()
        .map(|(k, v)| format!("{k:?}:{v:?}"))
        .collect();
    let mapping = MAPPING.replace('"', "\\\"");
    format!(
        r#"{{"name":"logs","uuid":"u1","number_of_shards":1,"mapping":"{mapping}","settings":{{{}}}}}"#,
        settings.join(",")
    )
}

pub(crate) struct Rig {
    pub(crate) server: oss_rest::Server,
    pub(crate) service: Arc<oss_node::search::SearchService<FsStore>>,
}

/// A published one-document shard of a pinned index, and a reader on it.
pub(crate) fn rig(root: &std::path::Path, dir: &ByteBuffersDirectory, wasm: &[u8]) -> Rig {
    rig_with(root, dir, wasm, (10_000_000, 10_000))
}

/// As [`rig`], the pin recording `limits` as (fuel, deadline in ms).
pub(crate) fn rig_with(
    root: &std::path::Path,
    dir: &ByteBuffersDirectory,
    wasm: &[u8],
    limits: (u64, u64),
) -> Rig {
    let writer_store = Arc::new(FsStore::new(root).expect("store"));
    let hash = extension_store::hash_of(wasm);
    extension_store::write_once(writer_store.as_ref(), "", &hash, wasm).expect("module");
    writer_store
        .create_register(
            "indices/logs",
            descriptor(&hash, limits.0, limits.1).as_bytes(),
        )
        .expect("descriptor");
    let mapping = Mapping::parse_pinned(MAPPING.as_bytes(), ParseOptions::default()).expect("map");
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(dir, w, mapping, 3, Box::new(|| 0))
        .expect("shard")
        .with_module("ext_a", Arc::new(Probe));
    shard
        .index(
            "1",
            br#"{"t":"anything at all"}"#,
            false,
            &Expect::default(),
        )
        .expect("indexed");
    shard.refresh().expect("refresh");
    let files = shard.committed_files().expect("files");
    let shard_ref = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    ShardPublisher::new(
        Arc::clone(&writer_store) as Arc<dyn BlobStore>,
        "",
        &shard_ref,
    )
    .expect("publisher")
    .publish(dir, &files, 3, "node-1", 0, 0)
    .expect("published");

    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = BufferPool::new(
        &MemoryBudget::new(&config),
        Pool::Transport,
        SMALL_BUFFER,
        16,
    )
    .expect("fits");
    let store = Arc::new(PooledStore::new(
        FsStore::new(root).expect("store"),
        buffers,
    ));
    let budget = MemoryBudget::new(&support::budget_config());
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let (server, service) =
        start_reader(listener, server_config(), store, "", "reader-1", &budget).expect("starts");
    Rig { server, service }
}

pub(crate) fn modules(root: &std::path::Path) -> Arc<ModuleSource> {
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    Arc::new(ModuleSource::new(
        Arc::new(FsStore::new(root).expect("store")) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        budget,
        16,
    ))
}

pub(crate) fn matched(rig: &Rig) -> support::Reply {
    request(
        rig.server.local_addr(),
        "POST",
        "/logs/_search",
        Some(br#"{"query":{"match":{"t":"zzzzzzz"}}}"#),
    )
}

// AC-13 (the match site)
#[test]
fn a_match_is_analysed_by_the_module_the_index_pins() {
    let root = TempDir::new("pinned-read");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    let reply = matched(&r);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply.text().contains(r#""value":1"#), "{}", reply.text());
}

/// Each query that analyses its text, over the one field the index pins: every text is the term
/// `probe` to the module, so each finds the document, and the built-in analyzer would find none.
pub(crate) const TEXT_QUERIES: [(&str, &str); 6] = [
    ("match", r#"{"match":{"t":"zzzzzzz"}}"#),
    ("match_phrase", r#"{"match_phrase":{"t":"zzzzzzz"}}"#),
    (
        "multi_match",
        r#"{"multi_match":{"query":"zzzzzzz","fields":["t"]}}"#,
    ),
    (
        "query_string",
        r#"{"query_string":{"query":"zzzzzzz","default_field":"t"}}"#,
    ),
    (
        "query_string field",
        r#"{"query_string":{"query":"t:zzzzzzz"}}"#,
    ),
    (
        "simple_query_string",
        r#"{"simple_query_string":{"query":"zzzzzzz","fields":["t"]}}"#,
    ),
];

// AC-13 (the other text queries)
#[test]
fn every_query_that_analyses_text_uses_the_module_the_index_pins() {
    let root = TempDir::new("pinned-sites");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    let missed: Vec<&str> = TEXT_QUERIES
        .iter()
        .filter(|(_, query)| {
            let body = format!(r#"{{"query":{query}}}"#);
            let reply = request(
                r.server.local_addr(),
                "POST",
                "/logs/_search",
                Some(body.as_bytes()),
            );
            reply.status != 200 || !reply.text().contains(r#""value":1"#)
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(missed.is_empty(), "not analysed by the module: {missed:?}");
}

// AC-13 (count)
#[test]
fn a_count_analyses_with_the_module_the_index_pins() {
    let root = TempDir::new("pinned-count");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    let counted = request(
        r.server.local_addr(),
        "POST",
        "/logs/_count",
        Some(br#"{"query":{"match":{"t":"zzzzzzz"}}}"#),
    );
    assert_eq!(counted.status, 200, "{}", counted.text());
    assert!(
        counted.text().contains(r#""count":1"#),
        "{}",
        counted.text()
    );
}

// AC-13 (msearch)
#[test]
fn an_msearch_analyses_with_the_module_the_index_pins() {
    let root = TempDir::new("pinned-msearch");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    let body = b"{\"index\":\"logs\"}\n{\"query\":{\"match\":{\"t\":\"zzzzzzz\"}}}\n";
    let many = request(r.server.local_addr(), "POST", "/_msearch", Some(body));
    assert_eq!(many.status, 200, "{}", many.text());
    assert!(many.text().contains(r#""value":1"#), "{}", many.text());
}

// AC-13 (_analyze)
#[test]
fn an_analyze_with_a_pins_name_runs_the_module_the_index_pins() {
    let root = TempDir::new("pinned-analyze");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    let said = request(
        r.server.local_addr(),
        "POST",
        "/logs/_analyze",
        Some(br#"{"analyzer":"ext_a","text":"zzzzzzz"}"#),
    );
    assert_eq!(said.status, 200, "{}", said.text());
    assert!(
        said.text().contains(r#""token":"probe""#),
        "{}",
        said.text()
    );
    assert!(
        said.text().contains(r#""end_offset":5"#),
        "the module's offsets: {}",
        said.text()
    );
    // A pin is the index's own: without the index the name is not found.
    let global = request(
        r.server.local_addr(),
        "POST",
        "/_analyze",
        Some(br#"{"analyzer":"ext_a","text":"zzzzzzz"}"#),
    );
    assert_eq!(global.status, 400, "{}", global.text());
}

// AC-14 (_analyze)
#[test]
fn an_analyze_with_a_pins_name_never_falls_back_when_the_module_cannot_load() {
    let root = TempDir::new("pinned-analyze-refused");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    let said = request(
        r.server.local_addr(),
        "POST",
        "/logs/_analyze",
        Some(br#"{"analyzer":"ext_a","text":"zzzzzzz"}"#),
    );
    assert_eq!(said.status, 503, "{}", said.text());
    assert!(!said.text().contains(r#""tokens""#), "{}", said.text());
}

// AC-14 (the negative cache on a booted node)
#[test]
fn a_module_the_store_lacks_is_asked_for_once_across_searches() {
    let root = TempDir::new("pinned-negative");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    // The reader's module store is empty: the pinned module is absent from it.
    let empty = Arc::new(oss_blob::Emulator::new(1 << 24, 5));
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    assert!(r.service.set_modules(Arc::new(ModuleSource::new(
        Arc::clone(&empty) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        budget,
        16,
    ))));
    let asked =
        || empty.meter().count(oss_blob::S3Op::Get) + empty.meter().count(oss_blob::S3Op::Head);
    let first = matched(&r);
    assert_eq!(first.status, 503, "{}", first.text());
    let spent = asked();
    assert!(spent > 0, "the first search asks the store for the module");
    for _ in 0..3 {
        let again = matched(&r);
        assert_eq!(again.status, 503, "{}", again.text());
    }
    assert_eq!(asked(), spent, "within 30 s the refusal is remembered");
}

// AC-14
#[test]
fn a_reader_that_cannot_load_the_module_refuses_the_search() {
    let root = TempDir::new("pinned-refused");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    let reply = matched(&r);
    assert_eq!(reply.status, 503, "{}", reply.text());
    assert!(
        reply.text().contains("extension_unavailable"),
        "{}",
        reply.text()
    );
}

// AC-13 (_validate/query): the module's answer decides validity. Its canned
// token ends at 5, so a text of 3 units is a malformed answer (invalid) and a
// text of 7 is a valid one; the built-in analyzer calls both valid.
#[test]
fn a_validate_query_analyses_with_the_module_the_index_pins() {
    let root = TempDir::new("pinned-validate");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    let valid = |text: &str| {
        let body = format!(r#"{{"query":{{"match":{{"t":"{text}"}}}}}}"#);
        let reply = request(
            r.server.local_addr(),
            "POST",
            "/logs/_validate/query",
            Some(body.as_bytes()),
        );
        assert_eq!(reply.status, 200, "{}", reply.text());
        reply.text().contains(r#""valid":true"#)
    };
    assert!(valid("zzzzzzz"));
    assert!(!valid("zzz"), "the module's token is past a 3-unit text");
}

// AC-13: a query that expands the terms of a field is refused on a field
// whose terms a module made, rather than answered from them.
#[test]
fn prefix_wildcard_regexp_and_fuzzy_are_refused_on_a_pinned_field() {
    let root = TempDir::new("pinned-expanding");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    for query in [
        r#"{"prefix":{"t":"zz"}}"#,
        r#"{"wildcard":{"t":"zz*"}}"#,
        r#"{"regexp":{"t":"zz.*"}}"#,
        r#"{"fuzzy":{"t":"zzzzzzy"}}"#,
    ] {
        let body = format!(r#"{{"query":{query}}}"#);
        let reply = request(
            r.server.local_addr(),
            "POST",
            "/logs/_search",
            Some(body.as_bytes()),
        );
        assert_eq!(reply.status, 400, "{query}: {}", reply.text());
        assert!(
            reply.text().contains("pins the analyzer [ext_a]"),
            "{query}: {}",
            reply.text()
        );
    }
}
