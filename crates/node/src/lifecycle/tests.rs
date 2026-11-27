use super::*;

fn lease(roles: &[&str]) -> NodeLease {
    let mut lease = NodeLease::new("n", "e", "a", 0);
    lease.set_roles(roles.iter().map(|r| (*r).to_owned()));
    lease
}

// docs/specs/node-ready-drain AC-4
#[test]
fn a_draining_leases_roles_lose_search_and_keep_the_rest() {
    let readiness = Readiness::new();
    let mut lease = lease(&["ingest", "search"]);
    readiness.stamp(&mut lease);
    assert_eq!(lease.roles(), ["ingest", "search"], "ready: untouched");
    assert!(readiness.start_draining());
    readiness.stamp(&mut lease);
    assert_eq!(lease.roles(), ["ingest"]);
    let mut other = self::lease(&["ingest"]);
    readiness.stamp(&mut other);
    assert_eq!(other.roles(), ["ingest"], "no search role: nothing to take");
}

// docs/specs/node-ready-drain AC-3
#[test]
fn draining_starts_once() {
    let readiness = Readiness::new();
    assert!(readiness.ready());
    assert!(readiness.start_draining());
    assert!(!readiness.start_draining());
    assert!(!readiness.ready());
}

// docs/specs/node-ready-drain AC-6
#[cfg(feature = "reader")]
#[test]
fn the_drain_settle_follows_the_lease_ttl_up_to_a_minute() {
    let settle = |ttl: &'static str| {
        crate::boot::NodeConfig::from_lookup(|key| {
            Some(
                match key {
                    "OSS_NODE_ID" => "n1",
                    "OSS_STORE" => "fs",
                    "OSS_FS_ROOT" => "/tmp/unused",
                    "OSS_LEASE_TTL_MS" => ttl,
                    _ => return None,
                }
                .to_owned(),
            )
        })
        .expect("valid")
        .drain_settle_millis
    };
    assert_eq!(settle("300"), 200);
    assert_eq!(settle("300000"), 60_000);
}

// docs/specs/reader-warm-copy AC-7
#[test]
fn a_warming_node_is_not_ready_and_has_no_search_role_until_it_finishes() {
    let readiness = Readiness::warming();
    assert!(!readiness.ready());
    assert_eq!(readiness.state(), "warming");
    let mut lease = lease(&["ingest", "search"]);
    readiness.stamp(&mut lease);
    assert_eq!(lease.roles(), ["ingest"], "no coordinator ranks it");
    readiness.finish_warming();
    assert!(readiness.ready());
    assert_eq!(readiness.state(), "ready");
    let mut again = self::lease(&["ingest", "search"]);
    readiness.stamp(&mut again);
    assert_eq!(again.roles(), ["ingest", "search"], "the role returns");
}

// docs/specs/reader-warm-copy AC-7
#[test]
fn a_drain_is_reported_over_warming_and_finishing_does_not_undo_it() {
    let readiness = Readiness::warming();
    assert!(readiness.start_draining());
    assert_eq!(readiness.state(), "draining");
    readiness.finish_warming();
    assert!(!readiness.ready(), "still draining");
    assert_eq!(Readiness::new().state(), "ready");
}

// docs/specs/reader-slow-start AC-8
#[test]
fn a_ready_leases_search_since_is_set_once_and_a_warming_one_has_none() {
    let warming = Readiness::warming();
    let mut l = lease(&["search"]);
    warming.stamp(&mut l);
    assert_eq!(l.search_since, None, "no search role was said");
    warming.finish_warming();
    let mut l = lease(&["search"]);
    warming.stamp(&mut l);
    let since = l.search_since.expect("stamped at ready");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut again = lease(&["search"]);
    warming.stamp(&mut again);
    assert_eq!(again.search_since, Some(since), "a renewal keeps it");
    let mut no_search = lease(&["ingest"]);
    warming.stamp(&mut no_search);
    assert_eq!(no_search.search_since, None);
}

// docs/specs/reader-slow-start AC-8a
#[test]
fn the_ramp_the_node_set_rides_in_its_lease() {
    let readiness = Readiness::new();
    let mut l = lease(&["search"]);
    readiness.stamp(&mut l);
    assert_eq!(l.ramp_permille(), None, "nothing said yet");
    readiness.set_ramp(300);
    let mut l = lease(&["search"]);
    readiness.stamp(&mut l);
    assert_eq!(l.ramp_permille(), Some(300));
}
