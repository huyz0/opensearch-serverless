//! docs/specs/node-index-route: create an index and ask whether one exists.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::DocKind;
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, DocSet, PitClock};
use oss_node::index_route::{IndexService, routes};
use oss_node::index_templates::TemplateSets;
use oss_rest::{Router, Server};

mod create_settings;

const BUF: usize = 1 << 16;
const NOW: u64 = 1_234_567;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    descriptors: DescriptorStore,
}

fn rig_with(budget: &MemoryBudget, buffer: usize) -> Rig {
    rig_knn(budget, buffer, false)
}

fn rig_knn(budget: &MemoryBudget, buffer: usize, knn: bool) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| NOW);
    let descriptors = || {
        DescriptorStore::new(
            Arc::clone(&store) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    };
    let service = Arc::new(IndexService::new(descriptors(), clock, buffer).with_knn(knn));
    let router = routes(Router::new(), service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(listener, support::server_config(), router, budget).expect("starts");
    Rig {
        server,
        descriptors: descriptors(),
        store,
    }
}

fn rig() -> Rig {
    rig_with(&MemoryBudget::new(&support::budget_config()), BUF)
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn ops(r: &Rig) -> Vec<(S3Op, u64)> {
    S3Op::ALL
        .iter()
        .map(|op| (*op, r.store.meter().count(*op)))
        .filter(|(_, n)| *n > 0)
        .collect()
}

fn record(r: &Rig, name: &str) -> Option<(String, u32, Option<String>, i64)> {
    let mut buf = vec![0u8; BUF];
    r.descriptors.get(name, &mut buf).expect("read").map(|rec| {
        (
            rec.uuid.to_string(),
            rec.number_of_shards,
            rec.mapping.as_deref().map(str::to_owned),
            rec.created_at_millis,
        )
    })
}

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"}}}"#;

#[test]
fn a_create_writes_the_descriptor_and_acknowledges() {
    let r = rig();
    let body = format!(
        r#"{{"settings":{{"number_of_shards":3,"number_of_replicas":0}},"mappings":{MAPPING}}}"#
    );
    let reply = call(&r, "PUT", "/logs", Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.text(),
        r#"{"acknowledged":true,"shards_acknowledged":true,"index":"logs","shards":3}"#
    );
    let (uuid, shards, mapping, created) = record(&r, "logs").expect("created");
    assert_eq!(uuid.len(), 22);
    assert_eq!(shards, 3);
    assert_eq!(mapping.as_deref(), Some(MAPPING));
    assert_eq!(created, i64::try_from(NOW).expect("small"));
    // A body that is only a mapping, and none at all.
    assert_eq!(call(&r, "PUT", "/bare", Some(MAPPING)).status, 200);
    assert_eq!(
        record(&r, "bare").expect("created").2.as_deref(),
        Some(MAPPING)
    );
    assert_eq!(call(&r, "PUT", "/empty", None).status, 200);
    assert_eq!(record(&r, "empty").expect("created").1, 1);
}

#[test]
fn a_second_create_of_a_name_is_refused_and_changes_nothing() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/logs", Some(MAPPING)).status, 200);
    let first = record(&r, "logs").expect("created");
    let reply = call(&r, "PUT", "/logs", None);
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(reply.text().contains("resource_already_exists_exception"));
    assert_eq!(record(&r, "logs"), Some(first));
}

#[test]
fn a_bad_name_is_refused_before_any_request() {
    let r = rig();
    for (name, status) in [("/_all", 501), ("/.ds-x", 400), ("/Upper", 400)] {
        let reply = call(&r, "PUT", name, None);
        assert_eq!(reply.status, status, "{name}: {}", reply.text());
    }
    assert!(
        call(&r, "PUT", "/Upper", None)
            .text()
            .contains("invalid_index_name_exception")
    );
    assert!(ops(&r).is_empty(), "{:?}", ops(&r));
}

#[test]
fn what_cannot_be_created_writes_nothing() {
    let r = rig();
    for (name, body, status) in [
        ("a", r#"{"settings":{"codec":"best_compression"}}"#, 501),
        ("f", r#"{"settings":{"refresh_interval":"fast"}}"#, 400),
        ("b", r#"{"aliases":{"x":{}}}"#, 501),
        ("c", "[1]", 400),
        ("d", r#"{"settings":{"number_of_shards":0}}"#, 400),
        (
            "e",
            r#"{"mappings":{"properties":{"f":{"type":"nonsense"}}}}"#,
            400,
        ),
    ] {
        let reply = call(&r, "PUT", &format!("/{name}"), Some(body));
        assert_eq!(reply.status, status, "{name}: {}", reply.text());
        assert!(record(&r, name).is_none(), "{name}");
    }
    assert!(
        ops(&r).iter().all(|(op, _)| *op != S3Op::Put),
        "{:?}",
        ops(&r)
    );
}

#[test]
fn head_says_whether_a_name_holds_an_index_or_an_alias() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/logs", None).status, 200);
    let reply = call(&r, "HEAD", "/logs", None);
    assert_eq!(reply.status, 200);
    assert!(reply.body.is_empty());
    let reply = call(&r, "HEAD", "/nothing", None);
    assert_eq!(reply.status, 404);
    assert!(reply.body.is_empty());
    let alias = oss_format::AliasRecord::new("a", &["logs"]);
    let mut buf = vec![0u8; BUF];
    r.descriptors
        .create_alias(&alias, 1, &mut buf)
        .expect("alias");
    assert_eq!(call(&r, "HEAD", "/a", None).status, 200);
}

#[test]
fn a_create_is_one_conditional_create_and_a_head_one_read() {
    let r = rig();
    let before = ops(&r);
    assert_eq!(call(&r, "PUT", "/logs", None).status, 200);
    let created: u64 =
        ops(&r).iter().map(|(_, n)| n).sum::<u64>() - before.iter().map(|(_, n)| n).sum::<u64>();
    assert_eq!(created, 1, "{:?}", ops(&r));
    let mid: u64 = ops(&r).iter().map(|(_, n)| n).sum();
    assert_eq!(call(&r, "HEAD", "/logs", None).status, 200);
    let head: u64 = ops(&r).iter().map(|(_, n)| n).sum::<u64>() - mid;
    assert_eq!(head, 1, "{:?}", ops(&r));
    assert!(ops(&r).iter().all(|(op, _)| *op != S3Op::List));
}

#[test]
fn a_pool_without_room_for_the_buffer_answers_429_and_writes_nothing() {
    let r = rig_with(&MemoryBudget::new(&support::budget_config()), 64 << 20);
    let reply = call(&r, "PUT", "/logs", None);
    assert_eq!(reply.status, 429, "{}", reply.text());
    assert!(ops(&r).is_empty(), "{:?}", ops(&r));
}

#[test]
fn a_body_of_one_mebibyte_is_read_and_one_byte_more_is_refused() {
    let r = rig_with(&MemoryBudget::new(&support::budget_config()), 2 << 20);
    let mut exact = String::from(MAPPING);
    exact.push_str(&" ".repeat((1 << 20) - MAPPING.len()));
    assert_eq!(exact.len(), 1 << 20);
    let reply = call(&r, "PUT", "/exact", Some(&exact));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let over = format!("{exact} ");
    let reply = call(&r, "PUT", "/over", Some(&over));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(record(&r, "over").is_none());
}

#[test]
fn a_descriptor_that_does_not_fit_its_buffer_is_refused_not_failed() {
    let r = rig();
    let mut big = String::from(MAPPING);
    big.push_str(&" ".repeat(BUF));
    let reply = call(&r, "PUT", "/big", Some(&big));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(record(&r, "big").is_none());
}

/// A rig whose creates place one-shard indexes in the bucket `b-<group>`.
type Left = Arc<std::sync::Mutex<Vec<(String, String)>>>;

fn bucketed_rig() -> (Rig, Left) {
    let seen: Left = Arc::default();
    let leaves = Arc::clone(&seen);
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| NOW);
    let descriptors = || {
        DescriptorStore::new(
            Arc::clone(&store) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    };
    let service = IndexService::new(descriptors(), clock, BUF).with_bucket_join(
        Arc::new(|group, _, _| Ok(format!("b-{group}"))),
        Arc::new(move |bucket, uuid| {
            leaves
                .lock()
                .unwrap()
                .push((bucket.to_owned(), uuid.to_owned()));
            Ok(())
        }),
    );
    let budget = MemoryBudget::new(&support::budget_config());
    let router = routes(Router::new(), Arc::new(service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    (
        Rig {
            server,
            descriptors: descriptors(),
            store,
        },
        seen,
    )
}

fn bucket_of(r: &Rig, name: &str) -> Option<String> {
    let mut buf = vec![0u8; BUF];
    let rec = r.descriptors.get(name, &mut buf).expect("read")?;
    rec.bucket.as_deref().map(str::to_owned)
}

#[test]
fn a_one_shard_create_joins_a_bucket_and_a_sharded_one_does_not() {
    let (r, _) = bucketed_rig();
    assert_eq!(call(&r, "PUT", "/small", None).status, 200);
    assert_eq!(bucket_of(&r, "small").as_deref(), Some("b-small"));
    let body = r#"{"settings":{"number_of_shards":3}}"#;
    assert_eq!(call(&r, "PUT", "/wide", Some(body)).status, 200);
    assert_eq!(bucket_of(&r, "wide"), None);
}

#[test]
fn a_create_names_its_group_by_the_bucket_group_setting() {
    let (r, _) = bucketed_rig();
    let body = r#"{"settings":{"index.serverless.bucket_group":"acme"}}"#;
    assert_eq!(call(&r, "PUT", "/a", Some(body)).status, 200);
    assert_eq!(bucket_of(&r, "a").as_deref(), Some("b-acme"));
}

fn tenancy_of_index(r: &Rig, name: &str) -> oss_node::writer::Tenancy {
    let mut buf = vec![0u8; BUF];
    let rec = r
        .descriptors
        .get(name, &mut buf)
        .expect("read")
        .expect("there");
    oss_node::writer::tenancy_of(rec.settings)
}

// docs/specs/tenant-tier-budgets AC-1
#[test]
fn a_create_keeps_its_group_tier_and_budget_and_refuses_a_bad_tier_or_budget() {
    let r = rig();
    let body = r#"{"settings":{"index.serverless.bucket_group":"acme","index.serverless.cost.tier":"small","index.serverless.cost.budget":"75"}}"#;
    assert_eq!(call(&r, "PUT", "/a", Some(body)).status, 200);
    let kept = tenancy_of_index(&r, "a");
    assert_eq!(kept.group.as_deref(), Some("acme"));
    assert_eq!(kept.tier.as_deref(), Some("small"));
    assert_eq!(kept.budget, Some(75));
    assert_eq!(call(&r, "PUT", "/plain", None).status, 200);
    assert!(tenancy_of_index(&r, "plain").is_empty());
    for bad in [
        r#"{"settings":{"index.serverless.cost.tier":"huge"}}"#,
        r#"{"settings":{"index.serverless.cost.budget":"0"}}"#,
        r#"{"settings":{"index.serverless.cost.budget":"lots"}}"#,
    ] {
        assert_eq!(call(&r, "PUT", "/bad", Some(bad)).status, 400, "{bad}");
    }
}

#[test]
fn an_index_that_sets_the_bucket_policy_off_joins_none() {
    let (r, _) = bucketed_rig();
    let body = r#"{"settings":{"index.serverless.bucket":"off"}}"#;
    assert_eq!(call(&r, "PUT", "/solo", Some(body)).status, 200);
    assert_eq!(bucket_of(&r, "solo"), None);
    let auto = r#"{"settings":{"index.serverless.bucket":"auto"}}"#;
    assert_eq!(call(&r, "PUT", "/grouped", Some(auto)).status, 200);
    assert_eq!(bucket_of(&r, "grouped").as_deref(), Some("b-grouped"));
}

#[test]
fn a_bad_bucket_policy_is_refused_and_other_settings_still_are() {
    let (r, _) = bucketed_rig();
    let bad = r#"{"settings":{"index.serverless.bucket":"maybe"}}"#;
    assert_eq!(call(&r, "PUT", "/x", Some(bad)).status, 400);
    let other = r#"{"settings":{"index.serverless.bucket_group":"t","index.codec":"x"}}"#;
    assert_eq!(call(&r, "PUT", "/y", Some(other)).status, 501);
}

#[test]
fn a_create_without_the_join_names_no_bucket() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/plain", None).status, 200);
    assert_eq!(bucket_of(&r, "plain"), None);
}

/// A rig whose creates inherit from the stored templates, each seeded as
/// (kind, name, source).
fn templated_rig(seed: &[(DocKind, &str, &str)]) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = || Arc::clone(&store) as Arc<dyn BlobStore>;
    let mut marker = vec![0u8; BUF];
    for (kind, name, source) in seed {
        DocSet::new(blobs(), "", *kind)
            .expect("set")
            .put(name, source.as_bytes(), false, &mut marker)
            .expect("seeded");
    }
    let clock: PitClock = Arc::new(|| NOW);
    let descriptors = || DescriptorStore::new(blobs(), "", DeleteMode::Conditional).expect("names");
    let sets = TemplateSets::new(
        DocSet::new(blobs(), "", DocKind::IndexTemplate).expect("set"),
        DocSet::new(blobs(), "", DocKind::ComponentTemplate).expect("set"),
    );
    let service = Arc::new(IndexService::new(descriptors(), clock, BUF).with_templates(sets));
    let router = routes(Router::new(), service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        descriptors: descriptors(),
        store,
    }
}

const LOGS_TEMPLATE: &str = r#"{"index_patterns":["logs-*"],"composed_of":["base"],"template":{"mappings":{"properties":{"msg":{"type":"text"}}}}}"#;
const BASE_COMPONENT: &str = r#"{"template":{"mappings":{"properties":{"ts":{"type":"date"}}}}}"#;

#[test]
fn a_create_inherits_the_mapping_and_shards_of_the_matching_template() {
    let r = templated_rig(&[
        (DocKind::IndexTemplate, "logs", LOGS_TEMPLATE),
        (DocKind::ComponentTemplate, "base", BASE_COMPONENT),
    ]);
    assert_eq!(call(&r, "PUT", "/logs-1", None).status, 200);
    let (_, _, mapping, _) = record(&r, "logs-1").expect("created");
    let mapping = mapping.expect("mapping");
    assert!(
        mapping.contains("\"msg\"") && mapping.contains("\"ts\""),
        "{mapping}"
    );
    assert_eq!(call(&r, "PUT", "/other", None).status, 200);
    assert_eq!(record(&r, "other").expect("created").2, None);
}

#[test]
fn a_create_with_no_template_stored_reads_only_the_marker() {
    let r = templated_rig(&[]);
    let before: u64 = ops(&r).iter().map(|(_, n)| n).sum();
    assert_eq!(call(&r, "PUT", "/logs", None).status, 200);
    let used = ops(&r).iter().map(|(_, n)| n).sum::<u64>() - before;
    assert_eq!(used, 2, "{:?}", ops(&r));
    assert!(ops(&r).iter().all(|(op, _)| *op != S3Op::List));
}

#[test]
fn a_create_that_fails_after_joining_leaves_the_bucket() {
    let (r, left) = bucketed_rig();
    assert_eq!(call(&r, "PUT", "/small", None).status, 200);
    assert!(left.lock().unwrap().is_empty(), "a good create stays in");
    assert_eq!(call(&r, "PUT", "/small", None).status, 400);
    let left = left.lock().unwrap();
    assert_eq!(left.len(), 1, "the refused create took itself back out");
    assert_eq!(left[0].0, "b-small");
    assert_eq!(left[0].1.len(), 22, "its own uuid, not the first create's");
}

const KNN_MAPPING: &str = r#"{"mappings":{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"name":"hnsw","engine":"lucene","space_type":"l2"}}}}}"#;

// docs/specs/knn-vector-index AC-4
#[test]
fn a_knn_vector_mapping_is_refused_unless_the_node_setting_is_on() {
    let off = rig();
    let refused = call(&off, "PUT", "/vec", Some(KNN_MAPPING));
    assert_ne!(refused.status, 200);
    assert!(record(&off, "vec").is_none());
    let on = rig_knn(&MemoryBudget::new(&support::budget_config()), BUF, true);
    let made = call(&on, "PUT", "/vec", Some(KNN_MAPPING));
    assert_eq!(made.status, 200, "{}", String::from_utf8_lossy(&made.body));
    assert!(record(&on, "vec").is_some());
}
