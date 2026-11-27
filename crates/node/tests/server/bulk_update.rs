//! docs/specs/node-bulk-update: the `update` action of `_bulk` over a writer
//! node's own shards.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::MemoryBudget;
use oss_node::bulk_route::{BulkService, routes as bulk_routes};
use oss_node::doc_route::{DocService, routes};
use oss_node::writer::{Routes, WriterRole};
use oss_rest::{Router, Server};
use oss_writer::bulk::{BulkBudget, StreamConfig};
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;

struct Lease;

impl EpochLease for Lease {
    fn record(&self, _: &[NodeLogEpoch]) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

/// One index, `docs`, of one shard held here; any other name is unknown.
struct Local;

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

impl Routes for Local {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "docs").then(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local: {}", group.len())
    }
}

/// One stored script, `bump`, and one that is not Painless, `expr`.
struct Stored;

impl oss_types::stored_scripts::StoredScripts for Stored {
    fn resolve(
        &self,
        id: &str,
    ) -> Result<oss_types::stored_scripts::Stored, oss_types::stored_scripts::ResolveError> {
        let (lang, source) = match id {
            "bump" => ("painless", "ctx._source.n += params.by"),
            "expr" => ("expression", "1"),
            _ => return Err(oss_types::stored_scripts::ResolveError::Missing),
        };
        Ok(oss_types::stored_scripts::Stored {
            lang: lang.to_owned(),
            source: source.to_owned(),
        })
    }
}

struct Rig {
    server: Server,
}

fn rig() -> Rig {
    let config = support::budget_config();
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log = NodeLogStore::new(store as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        7,
        Box::new(Arc::new(Lease)),
        Box::new(Arc::new(Admit)),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let role = Arc::new(WriterRole::new(committer, Arc::new(Local)));
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"},"m":{"type":"integer"},"o":{"properties":{"x":{"type":"integer"},"y":{"type":"integer"}}}}}"#).expect("mapping"),
    )
    .expect("opens");
    let service = Arc::new(DocService::new(Arc::clone(&role), [0; 6]));
    let router = routes(Router::new(), service).expect("routes");
    let bulk = Arc::new(
        BulkService::new(
            role,
            BulkBudget::new(1 << 20, 2),
            StreamConfig {
                slice_bytes: 1 << 20,
                max_wait: Duration::from_secs(5),
            },
            [0; 6],
        )
        .with_scripts(Arc::new(Stored)),
    );
    let router = bulk_routes(router, bulk).expect("bulk routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(
        listener,
        support::server_config(),
        router,
        &MemoryBudget::new(&config),
    )
    .expect("starts");
    Rig { server }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn put(r: &Rig, id: &str, source: &str) {
    let w = call(r, "PUT", &format!("/docs/_doc/{id}"), Some(source));
    assert!(w.status == 200 || w.status == 201, "{}", w.text());
}

fn bulk(r: &Rig, lines: &[&str]) -> String {
    let body = format!("{}\n", lines.join("\n"));
    let reply = call(r, "POST", "/docs/_bulk", Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    reply.text()
}

#[test]
fn bulk_updates_merge_skip_upsert_and_refuse() {
    let r = rig();
    put(&r, "1", r#"{"n":1,"o":{"x":1}}"#);
    put(&r, "2", r#"{"n":1}"#);
    put(&r, "6", r#"{"n":1}"#);
    put(&r, "7", r#"{"n":1}"#);
    let t = bulk(
        &r,
        &[
            r#"{"update":{"_id":"1"}}"#,
            r#"{"doc":{"m":5,"o":{"y":9}}}"#,
            r#"{"update":{"_id":"2"}}"#,
            r#"{"doc":{"n":1}}"#,
            r#"{"update":{"_id":"3"}}"#,
            r#"{"doc":{"n":1}}"#,
            r#"{"update":{"_id":"4"}}"#,
            r#"{"doc":{"n":1},"doc_as_upsert":true}"#,
            r#"{"update":{"_id":"5"}}"#,
            r#"{"doc":{"n":1},"upsert":{"n":2}}"#,
            r#"{"update":{"_id":"6","if_seq_no":99,"if_primary_term":3}}"#,
            r#"{"doc":{"n":2}}"#,
            r#"{"update":{"_id":"7"}}"#,
            r#"{"doc":{"n":2},"bogus":1}"#,
        ],
    );
    assert!(
        t.contains(r#""update":{"_index":"docs","_id":"1","_version":2,"result":"updated""#),
        "{t}"
    );
    assert!(
        t.contains(r#""_id":"2","_version":1,"result":"noop""#),
        "{t}"
    );
    assert!(t.contains(r#""_id":"3","status":404"#), "{t}");
    assert!(t.contains("document_missing_exception"), "{t}");
    assert!(
        t.contains(r#""_id":"4","_version":1,"result":"created""#),
        "{t}"
    );
    assert!(
        t.contains(r#""_id":"5","_version":1,"result":"created""#),
        "{t}"
    );
    assert!(t.contains("required seqNo [99]"), "{t}");
    assert!(t.contains("unknown field [bogus]"), "{t}");
    // the merged document is the one stored: the same merge is now a noop
    let again = bulk(
        &r,
        &[
            r#"{"update":{"_id":"1"}}"#,
            r#"{"doc":{"n":1,"m":5,"o":{"x":1,"y":9}}}"#,
        ],
    );
    assert!(again.contains(r#""result":"noop""#), "{again}");
}

#[test]
fn a_bulk_update_script_edits_deletes_or_is_refused_without_the_switch() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    let r = rig();
    put(&r, "1", r#"{"n":1}"#);
    put(&r, "2", r#"{"n":1}"#);
    let items = [
        r#"{"update":{"_id":"1"}}"#,
        r#"{"script":{"source":"ctx._source.n += params.by","params":{"by":4}}}"#,
        r#"{"update":{"_id":"2"}}"#,
        r#"{"script":{"source":"ctx.op = 'delete'"}}"#,
    ];
    oss_reader::painless_switch::set_native_compile(false);
    let off = bulk(&r, &items);
    assert!(off.contains(r#""status":501"#), "{off}");
    oss_reader::painless_switch::set_native_compile(true);
    let on = bulk(&r, &items);
    assert!(
        on.contains(r#""_id":"1","_version":2,"result":"updated""#),
        "{on}"
    );
    assert!(
        on.contains(r#""_id":"2","_version":2,"result":"deleted""#),
        "{on}"
    );
    let check = bulk(&r, &[r#"{"update":{"_id":"1"}}"#, r#"{"doc":{"n":5}}"#]);
    assert!(
        check.contains(r#""result":"noop""#),
        "the script left n=5: {check}"
    );
    oss_reader::painless_switch::set_native_compile(false);
}

#[test]
fn two_updates_of_one_document_in_one_bulk_conflict() {
    let r = rig();
    put(&r, "1", r#"{"n":1}"#);
    let t = bulk(
        &r,
        &[
            r#"{"update":{"_id":"1"}}"#,
            r#"{"doc":{"n":2}}"#,
            r#"{"update":{"_id":"1"}}"#,
            r#"{"doc":{"n":3}}"#,
        ],
    );
    assert!(t.contains(r#""result":"updated""#), "{t}");
    assert!(t.contains("an earlier item of this bulk"), "{t}");
}

// docs/specs/stored-script-ids AC-4, over a bulk item.
#[test]
fn a_bulk_update_runs_a_stored_script_by_id_and_refuses_what_an_id_may_not_be() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    let r = rig();
    put(&r, "1", r#"{"n":1}"#);
    oss_reader::painless_switch::set_native_compile(true);
    let ran = bulk(
        &r,
        &[
            r#"{"update":{"_id":"1"}}"#,
            r#"{"script":{"id":"bump","params":{"by":4}}}"#,
        ],
    );
    assert!(ran.contains(r#""result":"updated""#), "{ran}");
    let check = bulk(&r, &[r#"{"update":{"_id":"1"}}"#, r#"{"doc":{"n":5}}"#]);
    assert!(check.contains(r#""result":"noop""#), "n is 5: {check}");
    for (script, word) in [
        (
            r#"{"script":{"id":"missing"}}"#,
            "unable to find script [missing]",
        ),
        (r#"{"script":{"id":"expr"}}"#, "lang [expression]"),
        (
            r#"{"script":{"id":"bump","source":"1"}}"#,
            "takes no [source] or [lang]",
        ),
    ] {
        let refused = bulk(&r, &[r#"{"update":{"_id":"1"}}"#, script]);
        assert!(refused.contains(word), "{refused}");
    }
    oss_reader::painless_switch::set_native_compile(false);
}
