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
