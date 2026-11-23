//! docs/specs/node-cluster-visibility: what following the cluster register costs.

#![cfg(all(feature = "reader", feature = "writer"))]

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_meta::ClusterConfig;
use oss_node::cluster_live::ClusterFeed;
use oss_testkit::budget;
use oss_types::settings::Live;

#[test]
fn a_read_of_the_register_is_one_request() {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
    let feed = ClusterFeed::new(Arc::clone(&store), "", Arc::new(Live::new())).expect("feed");
    let config = ClusterConfig::new(Arc::clone(&store), "").expect("config");
    config
        .update(&[("a.b", Some("1"))], &mut vec![0u8; 1 << 16])
        .expect("updated");
    let before = store.meter().total();
    assert!(feed.read().expect("read"));
    budget::check(
        "cluster_settings.read.requests",
        store.meter().total() - before,
        "one register read",
    );
}
