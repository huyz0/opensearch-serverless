//! docs/specs/node-search-fresh: a search asks whether its shard holds every
//! refreshed write before it answers.

#[path = "support/mod.rs"]
mod support;

#[path = "overlay_search_rig/mod.rs"]
mod rig;

use std::sync::{Arc, Mutex};

use lucene_store::ByteBuffersDirectory;
use oss_node::overlay_builder::TailFetcher;
use oss_testkit::TempDir;
use oss_transport::actions::tail::{FetchRequest, FetchResponse};

use rig::*;
use support::request;

/// Answers every ask with a claim and no grant, counting the asks.
struct Claims {
    claimed: i64,
    asked: Mutex<u32>,
}

impl TailFetcher for Claims {
    fn fetch(&self, _: &str, request: &FetchRequest) -> Option<FetchResponse> {
        let mut asked = self.asked.lock().expect("lock");
        *asked = asked.saturating_add(1);
        Some(FetchResponse {
            records: Vec::new(),
            subscription: request.subscription,
            complete: false,
            claimed_through: self.claimed,
        })
    }
}

/// Installs a freshness check over a writer claiming `claimed`.
fn checked(fx: &Fixture<'_>, claimed: i64, grants: bool) -> Arc<Claims> {
    let claims = Arc::new(Claims {
        claimed,
        asked: Mutex::new(0),
    });
    let fresh = oss_node::tail_fresh::TailFresh::new(
        Arc::new(oss_node::tail_held::TailHeld::new()),
        Arc::clone(&claims) as Arc<dyn TailFetcher>,
        "reader-1".to_owned(),
        Arc::new(|| 1_000),
        grants,
    );
    assert!(fx.service.set_tail_fresh(Arc::new(fresh)));
    claims
}

fn asked(claims: &Claims) -> u32 {
    *claims.asked.lock().expect("lock")
}

#[test]
fn a_shard_behind_its_writers_claim_is_refused_as_a_shard_failure() {
    let root = TempDir::new("fresh-behind");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let claims = checked(&fx, i64::MAX, true);
    let reply = fx.search();
    assert_eq!(reply.status, 503);
    let body = String::from_utf8_lossy(&reply.body);
    assert!(body.contains("shard_behind_exception"), "{body}");
    assert!(body.contains(r#""node":"reader-1""#), "{body}");
    assert!(asked(&claims) >= 1);
}

#[test]
fn the_count_and_multi_search_routes_refuse_it_too() {
    let root = TempDir::new("fresh-behind-routes");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    checked(&fx, i64::MAX, true);
    let count = request(fx.server.local_addr(), "POST", "/logs/_count", None);
    assert_eq!(count.status, 503);
    let multi = request(
        fx.server.local_addr(),
        "POST",
        "/_msearch",
        Some(b"{\"index\":\"logs\"}\n{\"query\":{\"match_all\":{}}}\n"),
    );
    assert!(
        String::from_utf8_lossy(&multi.body).contains("shard_behind_exception"),
        "{}",
        String::from_utf8_lossy(&multi.body)
    );
}

#[test]
fn a_shard_at_its_writers_claim_answers() {
    let root = TempDir::new("fresh-caught-up");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let claims = checked(&fx, -1, true);
    assert_eq!(all(&fx), 2);
    assert!(asked(&claims) >= 1);
}

#[test]
fn a_shard_its_own_node_writes_and_a_node_without_grants_ask_nothing() {
    let root = TempDir::new("fresh-own");
    let dir = ByteBuffersDirectory::new();
    // The commit is staged by node-1.
    let fx = fixture_as(root.path(), &dir, &["a", "b"], "node-1");
    let claims = checked(&fx, i64::MAX, true);
    assert_eq!(all(&fx), 2);
    assert_eq!(asked(&claims), 0);

    let root = TempDir::new("fresh-off");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let claims = checked(&fx, i64::MAX, false);
    assert_eq!(all(&fx), 2);
    assert_eq!(asked(&claims), 0);
}
