//! docs/specs/ism slice 1: the policy store and its routes.

use crate::ism_rig::{POLICY, call, rig, total, with};

use oss_blob::S3Op;

// AC-1
#[test]
fn a_policy_is_created_read_replaced_listed_and_deleted() {
    let r = rig();
    let made = call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    assert_eq!(made.status, 201, "{}", made.text());
    let body: serde_json::Value = serde_json::from_str(&made.text()).unwrap();
    assert_eq!(body["_id"], "logs");
    assert_eq!(body["_version"], 1);
    assert_eq!(body["_seq_no"], 0);
    assert_eq!(body["_primary_term"], 1);
    assert_eq!(body["policy"]["policy_id"], "logs");
    assert_eq!(body["policy"]["default_state"], "hot");

    let read = call(&r, "GET", "/_plugins/_ism/policies/logs", None);
    assert_eq!(read.status, 200, "{}", read.text());
    let read: serde_json::Value = serde_json::from_str(&read.text()).unwrap();
    assert_eq!(read["policy"]["states"][0]["name"], "hot");

    // a replace without the sequence numbers is a conflict, as the plugin's
    let bare = call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    assert_eq!(bare.status, 409, "{}", bare.text());
    let stale = call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/logs?if_seq_no=7&if_primary_term=1",
        Some(POLICY),
    );
    assert_eq!(stale.status, 409, "{}", stale.text());
    let ok = call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/logs?if_seq_no=0&if_primary_term=1",
        Some(POLICY),
    );
    assert_eq!(ok.status, 200, "{}", ok.text());
    let ok: serde_json::Value = serde_json::from_str(&ok.text()).unwrap();
    assert_eq!(
        (ok["_seq_no"].as_i64(), ok["_version"].as_i64()),
        (Some(1), Some(2))
    );

    // the legacy prefix is the same tree
    let legacy = call(&r, "GET", "/_opendistro/_ism/policies/logs", None);
    assert_eq!(legacy.status, 200, "{}", legacy.text());

    call(&r, "PUT", "/_plugins/_ism/policies/another", Some(POLICY));
    let list = call(&r, "GET", "/_plugins/_ism/policies", None);
    assert_eq!(list.status, 200, "{}", list.text());
    let list: serde_json::Value = serde_json::from_str(&list.text()).unwrap();
    assert_eq!(list["total_policies"], 2);
    assert_eq!(list["policies"][0]["_id"], "another");

    let gone = call(&r, "DELETE", "/_plugins/_ism/policies/logs", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert!(gone.text().contains(r#""result":"deleted""#));
    let after = call(&r, "GET", "/_plugins/_ism/policies/logs", None);
    assert_eq!(after.status, 404, "{}", after.text());
    assert!(
        after.text().contains("Policy not found"),
        "{}",
        after.text()
    );
    let again = call(&r, "DELETE", "/_plugins/_ism/policies/logs", None);
    assert_eq!(again.status, 404, "{}", again.text());
}

// AC-1
#[test]
fn an_invalid_policy_is_refused_naming_what_is_wrong() {
    let r = rig();
    let cases = [
        (
            with(
                POLICY,
                "\"state_name\":\"warm\"",
                "\"state_name\":\"nowhere\"",
            ),
            "nowhere",
        ),
        (
            with(
                POLICY,
                "\"default_state\":\"hot\"",
                "\"default_state\":\"cold\"",
            ),
            "cold",
        ),
        (
            with(POLICY, "{\"read_only\":{}}", "{\"close\":{}}"),
            "close",
        ),
        (
            with(
                POLICY,
                "{\"read_only\":{}}",
                "{\"force_merge\":{\"max_num_segments\":1}}",
            ),
            "force_merge",
        ),
        (
            with(POLICY, "\"min_index_age\":\"1d\"", "\"min_doc_count\":5"),
            "min_doc_count",
        ),
        (
            with(POLICY, "\"min_index_age\":\"2d\"", "\"min_doc_count\":5"),
            "min_doc_count",
        ),
        (
            with(POLICY, "\"min_index_age\":\"1d\"", "\"min_size\":\"5gb\""),
            "min_size",
        ),
        (
            with(POLICY, "{\"name\":\"warm\"", "{\"name\":\"hot\""),
            "hot",
        ),
        (
            "{\"policy\":{\"default_state\":\"a\",\"states\":[]}}".to_owned(),
            "states",
        ),
        ("not json".to_owned(), "body"),
    ];
    for (body, word) in cases {
        let reply = call(&r, "PUT", "/_plugins/_ism/policies/bad", Some(&body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
        assert!(
            reply.text().contains("illegal_argument_exception") && reply.text().contains(word),
            "{word}: {}",
            reply.text()
        );
    }
    assert_eq!(
        call(&r, "GET", "/_plugins/_ism/policies/bad", None).status,
        404
    );
}

// AC-2
#[test]
fn a_policy_costs_the_stored_scripts_shape_and_nothing_lists() {
    let r = rig();
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 2),
        "a create"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", "/_plugins/_ism/policies/logs", None);
    assert_eq!(total(&r, S3Op::Get) - gets, 2, "a read");
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "DELETE", "/_plugins/_ism/policies/logs", None);
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 1),
        "a delete"
    );
    assert_eq!(total(&r, S3Op::List), 0);
}

// AC-1: the set holds at most 1,000 policies
#[test]
fn a_thousand_and_first_policy_is_refused() {
    let r = rig();
    for i in 0..1000 {
        let put = call(
            &r,
            "PUT",
            &format!("/_plugins/_ism/policies/p{i}"),
            Some(POLICY),
        );
        assert_eq!(put.status, 201, "{i}: {}", put.text());
    }
    let over = call(&r, "PUT", "/_plugins/_ism/policies/over", Some(POLICY));
    assert_eq!(over.status, 429, "{}", over.text());
}
