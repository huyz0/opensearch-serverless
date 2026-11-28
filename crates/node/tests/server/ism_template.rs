//! docs/specs/ism slice 2c: `ism_template` attaches a policy to an index at create.

use crate::ism_rig::{BUF, POLICY, Rig, call, json, total, with};
use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock};
use oss_node::index_route::{IndexService, routes as index_routes};
use oss_node::ism_route::{IsmService, routes};
use oss_rest::{Router, Server};

const KEY: &str = "index.plugins.index_state_management.policy_id";

pub(crate) fn rig(hooked: bool) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let descriptors = || {
        DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).expect("descriptors")
    };
    let ism = Arc::new(IsmService::new(&blobs, "", BUF).with_descriptors(descriptors()));
    let clock: PitClock = Arc::new(|| 1_700_000_000_000);
    let mut indices = IndexService::new(descriptors(), clock, BUF);
    if hooked {
        indices = indices.with_created(ism.on_create());
    }
    let router = index_routes(Router::new(), Arc::new(indices)).expect("index routes");
    let router = routes(router, &ism).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn templated(patterns: &str, priority: i64) -> String {
    with(
        POLICY,
        "\"description\":\"logs\",",
        &format!(
            "\"description\":\"logs\",\"ism_template\":{{\"index_patterns\":{patterns},\"priority\":{priority}}},"
        ),
    )
}

fn managed_by(r: &Rig, index: &str) -> serde_json::Value {
    let e = call(r, "GET", &format!("/_plugins/_ism/explain/{index}"), None);
    json(&e)[index][KEY].clone()
}

// AC-4
#[test]
fn a_created_index_that_matches_a_template_is_managed_and_priority_wins() {
    let r = rig(true);
    let put = |id: &str, body: &str| {
        let p = call(
            &r,
            "PUT",
            &format!("/_plugins/_ism/policies/{id}"),
            Some(body),
        );
        assert_eq!(p.status, 201, "{}", p.text());
    };
    put("low", &templated(r#"["logs-*"]"#, 1));
    put("high", &templated(r#"["logs-a*","other"]"#, 5));

    for (index, expected) in [
        ("logs-b1", "low"),
        ("logs-a1", "high"),
        ("other", "high"),
        ("plain", ""),
    ] {
        let c = call(&r, "PUT", &format!("/{index}"), Some("{}"));
        assert_eq!(c.status, 200, "{}", c.text());
        let got = managed_by(&r, index);
        if expected.is_empty() {
            assert!(got.is_null(), "{index}: {got}");
        } else {
            assert_eq!(got, expected, "{index}");
        }
    }
    let shown = json(&call(&r, "GET", "/_plugins/_ism/policies/high", None));
    assert_eq!(shown["policy"]["ism_template"]["priority"], 5);
}

#[test]
fn a_replace_without_a_template_and_a_delete_stop_the_attaching() {
    let r = rig(true);
    call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/p",
        Some(&templated(r#"["a-*"]"#, 1)),
    );
    let shown = json(&call(&r, "GET", "/_plugins/_ism/policies/p", None));
    let replace = format!(
        "/_plugins/_ism/policies/p?if_seq_no={}&if_primary_term=1",
        shown["_seq_no"]
    );
    let plain = call(&r, "PUT", &replace, Some(POLICY));
    assert_eq!(plain.status, 200, "{}", plain.text());
    call(&r, "PUT", "/a-1", Some("{}"));
    assert!(managed_by(&r, "a-1").is_null());

    call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/q",
        Some(&templated(r#"["b-*"]"#, 1)),
    );
    call(&r, "PUT", "/b-1", Some("{}"));
    assert_eq!(managed_by(&r, "b-1"), "q");
    assert_eq!(
        call(&r, "DELETE", "/_plugins/_ism/policies/q", None).status,
        200
    );
    call(&r, "PUT", "/b-2", Some("{}"));
    assert!(managed_by(&r, "b-2").is_null());
}

#[test]
fn an_overlapping_template_or_a_malformed_one_is_refused() {
    let r = rig(true);
    let ok = call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/p",
        Some(&templated(r#"["a-*"]"#, 3)),
    );
    assert_eq!(ok.status, 201);
    let clash = call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/q",
        Some(&templated(r#"["a-*"]"#, 3)),
    );
    assert_eq!(clash.status, 400, "{}", clash.text());
    assert_eq!(
        call(&r, "GET", "/_plugins/_ism/policies/q", None).status,
        404
    );
    for bad in [
        templated("[]", 1),
        templated(r#"["-a"]"#, 1),
        templated(r#"["a"]"#, 1).replace("\"priority\":1", "\"priority\":\"x\""),
    ] {
        let got = call(&r, "PUT", "/_plugins/_ism/policies/z", Some(&bad));
        assert_eq!(got.status, 400, "{}", got.text());
    }
}

#[test]
fn a_create_costs_one_more_get_when_no_template_exists_and_two_when_one_does() {
    let off = rig(false);
    let base = {
        let before = total(&off, S3Op::Get);
        call(&off, "PUT", "/plain", Some("{}"));
        total(&off, S3Op::Get) - before
    };
    let r = rig(true);
    let before = total(&r, S3Op::Get);
    call(&r, "PUT", "/plain", Some("{}"));
    assert_eq!(
        total(&r, S3Op::Get) - before,
        base + 1,
        "no templates: one GET"
    );

    call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/p",
        Some(&templated(r#"["a-*"]"#, 1)),
    );
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "PUT", "/zzz", Some("{}"));
    assert_eq!(total(&r, S3Op::Get) - gets, base + 2, "templates, no match");
    assert_eq!(total(&r, S3Op::Put) - puts, 1, "only the descriptor");
    assert_eq!(total(&r, S3Op::List), 0);
}
