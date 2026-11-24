//! docs/specs/writer-hand-off: a node started from configuration reports its
//! cap and what it holds in the lease it renews.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::FsStore;
use oss_cluster::{LeaseClock, LeaseConfig, Membership, SystemClock};
use oss_node::boot::{NodeConfig, start};
use oss_testkit::TempDir;

// docs/specs/writer-hand-off AC-7
#[test]
fn a_started_nodes_lease_reports_its_cap_and_what_it_holds() {
    let dir = TempDir::new("boot-lease-load");
    let root = dir.path().display().to_string();
    let config = NodeConfig::from_lookup(|key| {
        Some(
            match key {
                "OSS_NODE_ID" => "n1",
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                "OSS_LEASE_TTL_MS" => "300",
                "OSS_MAX_SHARDS" => "50",
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("valid");
    let _node = start(&config).expect("starts");
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let observer = Membership::new(
        Arc::new(FsStore::new(dir.path()).expect("store")),
        "",
        clock,
        LeaseConfig::default(),
    )
    .expect("membership");
    let started = Instant::now();
    loop {
        observer.refresh().expect("reads");
        if observer
            .current()
            .iter()
            .any(|l| l.node_id == "n1" && l.cap == 50 && l.held == 0)
        {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the cap never showed"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
