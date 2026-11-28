//! docs/specs/ism slice 2: attaching a policy to an index.

use crate::ism_rig::{ADD, BUF, POLICY, call, json, policy_logs2, rig, seed, total};
use std::sync::Arc;

use oss_blob::{BlobStore, S3Op};

// AC-3
#[test]
fn a_policy_is_added_to_an_index_explained_and_removed() {
    let r = rig();
    seed(&r, "logs-1", 1_700_000_000_000);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));

    let none = call(&r, "GET", "/_plugins/_ism/explain/logs-1", None);
    assert_eq!(none.status, 200, "{}", none.text());
    let none = json(&none);
    assert!(none["logs-1"]["index.plugins.index_state_management.policy_id"].is_null());
    assert_eq!(none["total_managed_indices"], 0);

    let added = call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));
    assert_eq!(added.status, 200, "{}", added.text());
    assert_eq!(
        json(&added),
        serde_json::json!({"updated_indices":1,"failures":false,"failed_indices":[]})
    );

    let explained = json(&call(&r, "GET", "/_plugins/_ism/explain/logs-1", None));
    let entry = &explained["logs-1"];
    assert_eq!(
        entry["index.plugins.index_state_management.policy_id"],
        "logs"
    );
    assert_eq!(
        entry["index.opendistro.index_state_management.policy_id"],
        "logs"
    );
    assert_eq!(entry["index"], "logs-1");
    assert_eq!(entry["index_uuid"], "uuid-logs-1");
    assert_eq!(entry["policy_id"], "logs");
    // before the runner's first step the plugin shows no state or sequence numbers
    assert!(entry["state"].is_null() && entry["policy_seq_no"].is_null());
    assert_eq!(entry["enabled"], true);
    assert_eq!(explained["total_managed_indices"], 1);

    // the legacy prefix and POST are the same route
    let legacy = call(&r, "POST", "/_opendistro/_ism/explain/logs-1", None);
    assert_eq!(legacy.status, 200, "{}", legacy.text());

    let again = json(&call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD)));
    assert_eq!(again["updated_indices"], 0);
    assert_eq!(again["failures"], true);
    assert_eq!(again["failed_indices"][0]["index_name"], "logs-1");
    assert!(
        again["failed_indices"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("already has a policy")
    );

    let removed = json(&call(&r, "POST", "/_plugins/_ism/remove/logs-1", None));
    assert_eq!(removed["updated_indices"], 1);
    let after = json(&call(&r, "GET", "/_plugins/_ism/explain/logs-1", None));
    assert!(after["logs-1"]["index.plugins.index_state_management.policy_id"].is_null());
    let twice = json(&call(&r, "POST", "/_plugins/_ism/remove/logs-1", None));
    assert_eq!(
        (
            twice["updated_indices"].as_i64(),
            twice["failures"].as_bool()
        ),
        (Some(0), Some(true))
    );
}

// AC-3
#[test]
fn an_add_that_cannot_attach_is_a_failure_entry_naming_why() {
    let r = rig();
    seed(&r, "logs-1", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    // a missing policy is the plugin's 404, a missing index is skipped and counted nowhere
    let nope = call(
        &r,
        "POST",
        "/_plugins/_ism/add/logs-1",
        Some(r#"{"policy_id":"nope"}"#),
    );
    assert_eq!(nope.status, 404, "{}", nope.text());
    assert!(nope.text().contains("Could not find policy=nope"));
    let ghost = json(&call(&r, "POST", "/_plugins/_ism/add/ghost", Some(ADD)));
    assert_eq!(
        ghost,
        serde_json::json!({"updated_indices":0,"failures":false,"failed_indices":[]})
    );
    let state = call(
        &r,
        "POST",
        "/_plugins/_ism/add/logs-1",
        Some(r#"{"policy_id":"logs","state":"cold"}"#),
    );
    assert!(
        json(&state)["failed_indices"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("not a state")
    );
    // a comma list adds each index it finds
    seed(&r, "logs-2", 2);
    let both = json(&call(
        &r,
        "POST",
        "/_plugins/_ism/add/logs-2,ghost,logs-1",
        Some(ADD),
    ));
    assert_eq!(both["updated_indices"], 2, "{both}");
    for (target, body) in [("logs-*", ADD), ("logs-1", "{}"), ("logs-1", "not json")] {
        let reply = call(
            &r,
            "POST",
            &format!("/_plugins/_ism/add/{target}"),
            Some(body),
        );
        assert_eq!(reply.status, 400, "{target} {body}: {}", reply.text());
    }
}

// AC-4b (slice 2: the record's cost; the run register and registry come with the runner)
#[test]
fn adding_reading_and_removing_a_policy_cost_their_requests_and_nothing_lists() {
    let r = rig();
    seed(&r, "logs-1", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (5, 3),
        "an add: the policy's 2 GETs, the index's 1, the record's 1 GET and 2 PUTs, the registry shard's 1 GET and 1 PUT"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", "/_plugins/_ism/explain/logs-1", None);
    assert_eq!(
        total(&r, S3Op::Get) - gets,
        2,
        "an explain of a managed index"
    );
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "POST", "/_plugins/_ism/remove/logs-1", None);
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (2, 1),
        "a remove: the index's record, the marker's read, the swap"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", "/_plugins/_ism/explain/logs-1", None);
    assert_eq!(
        total(&r, S3Op::Get) - gets,
        2,
        "an explain of an unmanaged index: the absent marker, then the index's record"
    );
    assert_eq!(total(&r, S3Op::List), 0);
}

// AC-11
#[test]
fn a_tenant_holds_at_most_a_thousand_managed_indices() {
    let r = rig();
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    for i in 0..=1000 {
        seed(&r, &format!("i{i}"), 1);
    }
    for i in 0..1000 {
        let add = call(&r, "POST", &format!("/_plugins/_ism/add/i{i}"), Some(ADD));
        assert_eq!(add.status, 200, "{i}: {}", add.text());
    }
    let over = call(&r, "POST", "/_plugins/_ism/add/i1000", Some(ADD));
    assert_eq!(over.status, 429, "{}", over.text());
}

// AC-3
#[test]
fn a_managed_index_is_asked_to_change_policy_and_an_unmanaged_one_is_not() {
    let r = rig();
    seed(&r, "logs-1", 1);
    seed(&r, "other", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/logs2",
        Some(&policy_logs2()),
    );
    call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));

    let change = r#"{"policy_id":"logs2","state":"warm"}"#;
    let done = json(&call(
        &r,
        "POST",
        "/_plugins/_ism/change_policy/logs-1",
        Some(change),
    ));
    assert_eq!(done["updated_indices"], 1, "{done}");
    let explained = json(&call(&r, "GET", "/_plugins/_ism/explain/logs-1", None));
    assert_eq!(
        explained["logs-1"]["policy_id"], "logs",
        "the runner applies it, not the call"
    );
    assert_eq!(explained["logs-1"]["change_policy"]["policy_id"], "logs2");
    assert_eq!(explained["logs-1"]["change_policy"]["state"], "warm");

    let cases = [
        ("other", change, "does not have a policy"),
        (
            "logs-1",
            r#"{"policy_id":"logs2","state":"cold"}"#,
            "not a state",
        ),
    ];
    for (index, body, word) in cases {
        let reply = json(&call(
            &r,
            "POST",
            &format!("/_plugins/_ism/change_policy/{index}"),
            Some(body),
        ));
        assert_eq!(reply["updated_indices"], 0, "{reply}");
        assert!(
            reply["failed_indices"][0]["reason"]
                .as_str()
                .unwrap()
                .contains(word),
            "{reply}"
        );
    }
    // a missing policy and a missing index are the plugin's 404s
    let nope = call(
        &r,
        "POST",
        "/_plugins/_ism/change_policy/logs-1",
        Some(r#"{"policy_id":"nope"}"#),
    );
    assert_eq!(nope.status, 404, "{}", nope.text());
    let ghost = call(
        &r,
        "POST",
        "/_plugins/_ism/change_policy/ghost",
        Some(change),
    );
    assert_eq!(ghost.status, 404, "{}", ghost.text());
    assert_eq!(
        call(
            &r,
            "POST",
            "/_plugins/_ism/change_policy/logs-*",
            Some(change)
        )
        .status,
        400
    );
}

// AC-8
#[test]
fn a_retry_is_taken_only_for_an_index_whose_step_failed() {
    let r = rig();
    seed(&r, "logs-1", 1);
    seed(&r, "other", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));
    // before the runner's first step the index has no metadata, as the plugin says
    let fresh = json(&call(&r, "POST", "/_plugins/_ism/retry/logs-1", None));
    assert_eq!(fresh["updated_indices"], 0, "{fresh}");
    assert!(
        fresh["failed_indices"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("no metadata information")
    );
    let unmanaged = json(&call(&r, "POST", "/_plugins/_ism/retry/other", None));
    assert_eq!(unmanaged["updated_indices"], 0, "{unmanaged}");
    let ghost = call(&r, "POST", "/_plugins/_ism/retry/ghost", None);
    assert_eq!(ghost.status, 404, "{}", ghost.text());

    // the runner marks a failed step; here the record is written as it would
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let set = oss_meta::DocSet::new(blobs, "", oss_format::DocKind::IsmManaged).unwrap();
    let mut marker = vec![0u8; BUF];
    let mut doc = vec![0u8; 1 << 20];
    let n = set.get("logs-1", &mut marker, &mut doc).unwrap().unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&doc[..n]).unwrap();
    record["failed"] = serde_json::json!(false);
    set.put("logs-1", record.to_string().as_bytes(), false, &mut marker)
        .unwrap();
    let healthy = json(&call(&r, "POST", "/_plugins/_ism/retry/logs-1", None));
    assert!(
        healthy["failed_indices"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("not in a failed state")
    );
    record["failed"] = serde_json::json!(true);
    set.put("logs-1", record.to_string().as_bytes(), false, &mut marker)
        .unwrap();

    let retried = json(&call(
        &r,
        "POST",
        "/_plugins/_ism/retry/logs-1",
        Some(r#"{"state":"warm"}"#),
    ));
    assert_eq!(retried["updated_indices"], 1, "{retried}");
    let explained = json(&call(&r, "GET", "/_plugins/_ism/explain/logs-1", None));
    assert_eq!(explained["logs-1"]["retry_requested"], true);
}

// AC-10
#[test]
fn a_change_policy_and_a_retry_cost_their_reads_and_one_put() {
    let r = rig();
    seed(&r, "logs-1", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/logs2",
        Some(&policy_logs2()),
    );
    call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(
        &r,
        "POST",
        "/_plugins/_ism/change_policy/logs-1",
        Some(r#"{"policy_id":"logs2"}"#),
    );
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (6, 2),
        "the policy's 2 GETs, the index's 1, the record's 2, the replace's 1 GET and 2 PUTs"
    );
    assert_eq!(total(&r, S3Op::List), 0);
}

// AC-5: an add names the tenant in the `ism` registry, so the runner finds it
#[test]
fn an_add_names_the_tenant_in_the_ism_registry() {
    let r = rig();
    seed(&r, "logs-1", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    let registry = || {
        let store: Arc<dyn BlobStore> = Arc::clone(&r.store) as Arc<dyn BlobStore>;
        oss_meta::ScheduleRegistry::in_namespace(store, "", "ism").expect("a registry")
    };
    assert!(!registry().is_registered("_default_").unwrap());
    call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));
    assert!(registry().is_registered("_default_").unwrap());
}
