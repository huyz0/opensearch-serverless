//! docs/specs/index-create-settings: the settings a create carries.

use super::*;

fn settings_text(r: &Rig, name: &str) -> String {
    let mut buf = vec![0u8; BUF];
    let rec = r
        .descriptors
        .get(name, &mut buf)
        .expect("read")
        .expect("there");
    String::from_utf8_lossy(rec.settings).into_owned()
}

// docs/specs/index-create-settings AC-1 and AC-6
#[test]
fn a_create_stores_the_settings_the_node_reads() {
    let r = rig();
    let body = r#"{"settings":{"number_of_shards":4,"index":{"refresh_interval":"5s"},"routing_partition_size":2,"number_of_routing_shards":8,"index.serverless.cost.tier":"small"}}"#;
    let reply = call(&r, "PUT", "/s", Some(body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let text = settings_text(&r, "s");
    assert_eq!(
        text,
        r#"{"index.serverless.cost.tier":"small","index.number_of_routing_shards":8,"index.refresh_interval":"5s","index.routing_partition_size":2}"#
    );
    assert_eq!(
        oss_node::writer::publish_age_of(text.as_bytes()),
        oss_writer::publish::lazy::PublishAge::Millis(5_000)
    );
    assert_eq!(
        oss_node::writer::tenancy_of(text.as_bytes())
            .tier
            .as_deref(),
        Some("small")
    );
}

// docs/specs/index-create-settings AC-2
#[test]
fn a_bad_refresh_interval_is_refused_at_create() {
    let r = rig();
    for (i, bad) in ["fast", "1", "-2", "1.5s"].iter().enumerate() {
        let body = format!(r#"{{"settings":{{"refresh_interval":"{bad}"}}}}"#);
        let reply = call(&r, "PUT", &format!("/r{i}"), Some(&body));
        assert_eq!(reply.status, 400, "{bad}: {}", reply.text());
        assert!(
            reply.text().contains("refresh_interval"),
            "{}",
            reply.text()
        );
        assert!(record(&r, &format!("r{i}")).is_none());
    }
    for (i, good) in ["0", "-1", "500ms"].iter().enumerate() {
        let body = format!(r#"{{"settings":{{"refresh_interval":"{good}"}}}}"#);
        assert_eq!(
            call(&r, "PUT", &format!("/g{i}"), Some(&body)).status,
            200,
            "{good}"
        );
    }
}

// docs/specs/index-create-settings AC-3
#[test]
fn routing_settings_are_checked_against_the_shard_count() {
    let r = rig();
    for (i, (shards, key, value, ok)) in [
        (1, "routing_partition_size", 1, true),
        (1, "routing_partition_size", 2, false),
        (4, "routing_partition_size", 3, true),
        (4, "routing_partition_size", 4, false),
        (4, "routing_partition_size", 0, false),
        (4, "number_of_routing_shards", 8, true),
        (4, "number_of_routing_shards", 6, false),
        (4, "number_of_routing_shards", 2, false),
    ]
    .into_iter()
    .enumerate()
    {
        let body = format!(r#"{{"settings":{{"number_of_shards":{shards},"{key}":{value}}}}}"#);
        let reply = call(&r, "PUT", &format!("/x{i}"), Some(&body));
        assert_eq!(
            reply.status,
            if ok { 200 } else { 400 },
            "{body}: {}",
            reply.text()
        );
        assert_eq!(record(&r, &format!("x{i}")).is_some(), ok, "{body}");
    }
}

// docs/specs/index-create-settings AC-4
#[test]
fn other_settings_are_refused_and_named() {
    let r = rig();
    let body =
        r#"{"settings":{"blocks.write":"true","refresh_interval":"1s","index.analysis.a":"y"}}"#;
    let reply = call(&r, "PUT", "/o", Some(body));
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(
        reply
            .text()
            .contains("[index.analysis.a, index.blocks.write]"),
        "{}",
        reply.text()
    );
    assert!(record(&r, "o").is_none());
}

// docs/specs/index-create-settings AC-5
#[test]
fn a_templates_settings_apply_and_the_request_wins() {
    let r = templated_rig(&[(
        DocKind::IndexTemplate,
        "logs",
        r#"{"index_patterns":["logs-*"],"template":{"settings":{"index":{"refresh_interval":"5s","routing_partition_size":1}}}}"#,
    )]);
    assert_eq!(call(&r, "PUT", "/logs-1", None).status, 200);
    assert_eq!(
        settings_text(&r, "logs-1"),
        r#"{"index.refresh_interval":"5s","index.routing_partition_size":1}"#
    );
    let own = r#"{"settings":{"refresh_interval":"1s"}}"#;
    assert_eq!(call(&r, "PUT", "/logs-2", Some(own)).status, 200);
    assert_eq!(
        settings_text(&r, "logs-2"),
        r#"{"index.refresh_interval":"1s","index.routing_partition_size":1}"#
    );
}

// docs/specs/index-create-settings AC-4
#[test]
fn a_template_setting_the_node_does_not_read_is_refused_not_dropped() {
    let r = templated_rig(&[(
        DocKind::IndexTemplate,
        "logs",
        r#"{"index_patterns":["logs-*"],"template":{"settings":{"index":{"analysis":{"a":"x"}}}}}"#,
    )]);
    let reply = call(&r, "PUT", "/logs-1", None);
    assert_eq!(reply.status, 501);
    assert!(record(&r, "logs-1").is_none());
}

// docs/specs/core-index-settings AC-1 and AC-2
#[test]
fn core_settings_the_node_only_stores_are_accepted_and_kept() {
    let r = rig();
    let body = r#"{"settings":{"index":{"codec":"best_compression","priority":5,"translog":{"durability":"async"},"routing.allocation.require.box":"hot","search.slowlog.threshold.query.warn":"10s"},"refresh_interval":"2s"}}"#;
    let reply = call(&r, "PUT", "/c", Some(body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        settings_text(&r, "c"),
        r#"{"index.codec":"best_compression","index.priority":5,"index.refresh_interval":"2s","index.routing.allocation.require.box":"hot","index.search.slowlog.threshold.query.warn":"10s","index.translog.durability":"async"}"#
    );
}

// docs/specs/core-index-settings AC-3
#[test]
fn a_bad_core_value_is_refused_and_writes_nothing() {
    let r = rig();
    for (i, body) in [
        r#"{"settings":{"index.codec":"gzip"}}"#,
        r#"{"settings":{"index.priority":"high"}}"#,
        r#"{"settings":{"index.queries.cache.enabled":"yes"}}"#,
    ]
    .iter()
    .enumerate()
    {
        let name = format!("b{i}");
        let reply = call(&r, "PUT", &format!("/{name}"), Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
        assert!(record(&r, &name).is_none());
    }
}

// docs/specs/core-index-settings AC-4
#[test]
fn a_core_setting_that_would_change_behaviour_is_still_refused() {
    let r = rig();
    let body = r#"{"settings":{"index.blocks.write":true,"index.codec":"lz4","index.max_result_window":50}}"#;
    let reply = call(&r, "PUT", "/x", Some(body));
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(
        reply
            .text()
            .contains("[index.blocks.write, index.max_result_window]"),
        "{}",
        reply.text()
    );
    assert!(record(&r, "x").is_none());
}
