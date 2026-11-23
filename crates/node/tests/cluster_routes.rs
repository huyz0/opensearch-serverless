//! docs/specs/node-cluster-routes: cluster settings and cluster and cat health over REST.

#[path = "cluster_support/mod.rs"]
mod cluster_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cluster_support::{call, get, health_of, rig};
use oss_coordinator::cluster::health::HeadRead;

#[test]
fn a_settings_read_nests_or_flattens_the_registers_persistent_settings() {
    let r = rig();
    let never = get(&r, "/_cluster/settings");
    assert_eq!(never.status, 200, "{}", never.text());
    assert_eq!(never.text(), r#"{"persistent":{},"transient":{}}"#);
    let put = call(
        &r,
        "PUT",
        "/_cluster/settings",
        Some(r#"{"persistent":{"serverless":{"bucket":"auto"},"serverless.tail_overlay":"true"}}"#),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    assert_eq!(
        get(&r, "/_cluster/settings").text(),
        r#"{"persistent":{"serverless":{"bucket":"auto","tail_overlay":"true"}},"transient":{}}"#
    );
    assert_eq!(
        get(
            &r,
            "/_cluster/settings?flat_settings=true&include_defaults=true"
        )
        .text(),
        r#"{"persistent":{"serverless.bucket":"auto","serverless.tail_overlay":"true"},"transient":{},"defaults":{}}"#
    );
}

#[test]
fn an_update_answers_what_the_register_holds_and_merges_and_removes() {
    let r = rig();
    let first = call(
        &r,
        "PUT",
        "/_cluster/settings",
        Some(r#"{"persistent":{"serverless.bucket":"auto","a.b":1}}"#),
    );
    assert_eq!(
        first.text(),
        r#"{"acknowledged":true,"persistent":{"a":{"b":"1"},"serverless":{"bucket":"auto"}},"transient":{}}"#
    );
    let second = call(
        &r,
        "PUT",
        "/_cluster/settings?flat_settings=true",
        Some(r#"{"persistent":{"serverless.bucket":null,"c":true}}"#),
    );
    assert_eq!(
        second.text(),
        r#"{"acknowledged":true,"persistent":{"a.b":"1","c":"true"},"transient":{}}"#
    );
}

#[test]
fn a_refused_update_answers_the_coordinators_words_and_writes_nothing() {
    let r = rig();
    let cases: [(Option<&str>, u16, &str, &str); 6] = [
        (
            None,
            400,
            "illegal_argument_exception",
            "a settings update needs a 'persistent' object",
        ),
        (
            Some(r#"{"transient":{"a":"b"}}"#),
            400,
            "illegal_argument_exception",
            "a settings update needs a 'persistent' object",
        ),
        (
            Some(r#"{"persistent":{"a":"b"},"transient":{"c":"d"}}"#),
            501,
            "status_exception",
            "transient settings are not supported; use persistent",
        ),
        (
            Some(r#"{"persistent":{"a":["b"]}}"#),
            501,
            "status_exception",
            "'a' is a list; list-valued settings are not supported on this register",
        ),
        (
            Some(r#"{"persistent":{"serverless.bucket":"sometimes"}}"#),
            400,
            "status_exception",
            "bucket must be [auto] or [off], got [sometimes]",
        ),
        (
            // Below the larger of 1,000 ms and twice the 250 ms commit period.
            Some(r#"{"persistent":{"serverless.wal.node_log.commits.checkpoint_millis":"10"}}"#),
            400,
            "status_exception",
            "checkpoint_millis",
        ),
    ];
    for (body, status, kind, words) in cases {
        let reply = call(&r, "PUT", "/_cluster/settings", body);
        let text = reply.text();
        assert_eq!(reply.status, status, "{body:?}: {text}");
        assert!(text.contains(kind), "{body:?}: {text}");
        assert!(text.contains(words), "{body:?}: {text}");
    }
    assert_eq!(
        get(&r, "/_cluster/settings").text(),
        r#"{"persistent":{},"transient":{}}"#
    );
}

#[test]
fn an_unscoped_health_answers_the_node_half_and_says_it_is_partial() {
    let r = rig();
    let reply = get(&r, "/_cluster/health");
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    for part in [
        r#""cluster_name":"test-cluster""#,
        r#""status":"green""#,
        r#""status_scope":"control_plane""#,
        r#""timed_out":false"#,
        r#""number_of_nodes":3"#,
        r#""number_of_data_nodes":3"#,
        r#""complete":false"#,
        r#""replication":"object-store""#,
        r#""yellow_reachable":false"#,
    ] {
        assert!(text.contains(part), "{part}: {text}");
    }
    assert!(!text.contains("active_shards"), "{text}");
    assert!(!text.contains("\"indices\""), "{text}");
}

#[test]
fn a_scoped_health_counts_each_indexs_shards_and_their_states() {
    let r = rig();
    health_of(
        &r,
        "logs",
        &[
            (0, HeadRead::Owned { held: true }),
            (1, HeadRead::Unowned),
            (2, HeadRead::Owned { held: false }),
        ],
    );
    health_of(&r, "docs", &[(0, HeadRead::Owned { held: true })]);
    let text = get(&r, "/_cluster/health/logs,docs").text();
    for part in [
        r#""status":"green""#,
        r#""complete":true"#,
        r#""active_primary_shards":4"#,
        r#""active_shards":4"#,
        r#""unassigned_shards":0"#,
        r#""dormant_shards":2"#,
        r#""active_shards_percent_as_number":100.0"#,
        r#""active_shards_percent":"100.0%""#,
        r#""indices":{"logs":{"status":"green","number_of_shards":3,"number_of_replicas":0,"active_primary_shards":3,"active_shards":3,"relocating_shards":0,"initializing_shards":0,"unassigned_shards":0,"dormant_shards":2}"#,
    ] {
        assert!(text.contains(part), "{part}: {text}");
    }
    assert!(!text.contains("\"shards\":{"), "{text}");
    let shards = get(&r, "/_cluster/health/logs?level=shards").text();
    assert!(
        shards.contains(
            r#""shards":{"0":{"status":"green","state":"owned","number_of_replicas":0},"1":{"status":"green","state":"dormant","number_of_replicas":0},"2":{"status":"green","state":"dormant","number_of_replicas":0}}"#
        ),
        "{shards}"
    );
}

#[test]
fn a_missing_index_is_404_and_what_cannot_be_read_is_red_not_an_error() {
    let r = rig();
    let missing = get(&r, "/_cluster/health/nope");
    assert_eq!(missing.status, 404, "{}", missing.text());
    assert!(
        missing.text().contains("index_not_found"),
        "{}",
        missing.text()
    );
    assert!(
        missing.text().contains("no such index: nope"),
        "{}",
        missing.text()
    );

    health_of(
        &r,
        "logs",
        &[
            (0, HeadRead::Owned { held: true }),
            (1, HeadRead::Unreadable),
        ],
    );
    let red = get(&r, "/_cluster/health/logs?level=shards");
    let text = red.text();
    assert_eq!(red.status, 200, "{text}");
    assert!(text.contains(r#""status":"red""#), "{text}");
    assert!(text.contains(r#""unassigned_shards":1"#), "{text}");
    assert!(text.contains(r#""state":"unservable""#), "{text}");
    assert!(
        text.contains(r#""active_shards_percent":"50.0%""#),
        "{text}"
    );

    r.fake.unreadable.store(true, Ordering::SeqCst);
    let down = get(&r, "/_cluster/health");
    assert_eq!(down.status, 200, "{}", down.text());
    assert!(down.text().contains(r#""status":"red""#), "{}", down.text());
}

#[test]
fn wait_for_nodes_waits_and_a_wait_that_ran_out_is_408() {
    let r = rig();
    let started = Instant::now();
    let out = get(&r, "/_cluster/health?wait_for_nodes=5&timeout=300ms");
    assert_eq!(out.status, 408, "{}", out.text());
    assert!(out.text().contains(r#""timed_out":true"#), "{}", out.text());
    assert!(started.elapsed() >= Duration::from_millis(300));

    for (condition, ok) in [
        ("3", true),
        (">=3", true),
        ("ge(3)", true),
        (">3", false),
        ("<=3", true),
        ("<3", false),
        ("lt(4)", true),
        ("gt(3)", false),
    ] {
        let reply = get(
            &r,
            &format!("/_cluster/health?wait_for_nodes={condition}&timeout=0ms"),
        );
        assert_eq!(reply.status == 200, ok, "{condition}: {}", reply.text());
    }

    let fake = Arc::clone(&r.fake);
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        fake.nodes.store(5, Ordering::SeqCst);
    });
    let waited = get(&r, "/_cluster/health?wait_for_nodes=5&timeout=10s");
    raiser.join().expect("joins");
    assert_eq!(waited.status, 200, "{}", waited.text());
    assert!(waited.text().contains(r#""number_of_nodes":5"#));

    let bad = get(&r, "/_cluster/health?wait_for_nodes=many");
    assert_eq!(bad.status, 400, "{}", bad.text());
    assert!(bad.text().contains("invalid_parameter"), "{}", bad.text());
    let unscoped = get(&r, "/_cluster/health?wait_for_status=green");
    assert_eq!(unscoped.status, 400, "{}", unscoped.text());
    assert!(unscoped.text().contains("unscoped_wait_for_status"));

    health_of(&r, "logs", &[(0, HeadRead::Unreadable)]);
    let red = get(
        &r,
        "/_cluster/health/logs?wait_for_status=green&timeout=0ms",
    );
    assert_eq!(red.status, 408, "{}", red.text());
}

#[test]
fn the_parameters_the_shell_refuses_are_501_and_its_hints_are_accepted() {
    let r = rig();
    for (query, words) in [
        (
            "wait_for_status=red",
            "wait_for_status must be 'green' or 'yellow'",
        ),
        (
            "wait_for_active_shards=all",
            "wait_for_active_shards counts shards",
        ),
        ("wait_for_events=urgent", "wait_for_events waits on"),
        (
            "level=nodes",
            "level must be 'cluster', 'indices' or 'shards'",
        ),
    ] {
        let reply = get(&r, &format!("/_cluster/health?{query}"));
        assert_eq!(reply.status, 501, "{query}: {}", reply.text());
        assert!(reply.text().contains("unsupported_health_parameter"));
        assert!(reply.text().contains(words), "{query}: {}", reply.text());
    }
    for hint in [
        "local=true",
        "master_timeout=1s",
        "cluster_manager_timeout=1s",
        "expand_wildcards=all",
        "awareness_attribute=zone",
        "ensure_node_weighed_in=true",
        "wait_for_no_relocating_shards=true",
        "wait_for_no_initializing_shards=true",
    ] {
        let reply = get(&r, &format!("/_cluster/health?{hint}"));
        assert_eq!(reply.status, 200, "{hint}: {}", reply.text());
    }
}

#[test]
fn cat_health_is_one_row_in_text_or_json() {
    let r = rig();
    let text = get(&r, "/_cat/health");
    assert_eq!(text.status, 200);
    assert_eq!(text.text(), "test-cluster green 3 3 - -\n");
    assert!(
        text.header("content-type")
            .is_some_and(|t| t.starts_with("text/plain")),
        "{:?}",
        text.header("content-type")
    );
    assert_eq!(
        get(&r, "/_cat/health?v=true").text(),
        "cluster      status node.total node.data shards dormant\ntest-cluster green  3          3         -      -\n"
    );
    // The cat route names no index, so it counts no shard.
    let by_index = get(&r, "/_cat/health/logs");
    assert_eq!(by_index.status, 400, "{}", by_index.text());
    let named = get(&r, "/_cat/health?format=json");
    assert_eq!(
        named.text(),
        r#"[{"cluster":"test-cluster","status":"green","node.total":"3","node.data":"3","shards":"-","dormant":"-"}]"#
    );
    assert!(
        named
            .header("content-type")
            .is_some_and(|t| t.starts_with("application/json"))
    );
    assert_eq!(get(&r, "/_cat/health?h=status").status, 501);
    assert_eq!(get(&r, "/_cat/health?s=status").status, 501);
}

/// A valid settings update of exactly `len` bytes: the object, then spaces.
fn update_of(len: usize) -> String {
    let mut body = String::from(r#"{"persistent":{"a.b":1}}"#);
    body.push_str(&" ".repeat(len.saturating_sub(body.len())));
    body
}

const MAX_SETTINGS_BODY: usize = 1 << 20;

#[test]
fn a_settings_body_of_the_limit_is_taken_and_one_byte_over_is_413() {
    let r = rig();
    let at = call(
        &r,
        "PUT",
        "/_cluster/settings",
        Some(&update_of(MAX_SETTINGS_BODY)),
    );
    assert_eq!(at.status, 200, "the limit itself is allowed");
    let over = call(
        &r,
        "PUT",
        "/_cluster/settings",
        Some(&update_of(MAX_SETTINGS_BODY + 1)),
    );
    assert_eq!(over.status, 413, "{}", over.text());
}

#[test]
fn a_settings_body_with_no_declared_length_is_bounded_as_it_is_read() {
    let r = rig();
    let at = update_of(MAX_SETTINGS_BODY);
    let (a, b) = at.as_bytes().split_at(MAX_SETTINGS_BODY / 2);
    assert_eq!(
        cluster_support::chunked_status(&r, "PUT", "/_cluster/settings", &[a, b]),
        200,
        "the limit itself is allowed"
    );
    let over = update_of(MAX_SETTINGS_BODY + 1);
    let (a, b) = over.as_bytes().split_at(MAX_SETTINGS_BODY / 2);
    assert_eq!(
        cluster_support::chunked_status(&r, "PUT", "/_cluster/settings", &[a, b]),
        413
    );
}
