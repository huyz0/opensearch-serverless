//! docs/specs/write-backpressure: the limiter's boot variables.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use oss_node::boot::NodeConfig;

// docs/specs/write-backpressure: the limiter's variables
#[test]
fn the_write_limit_is_read_from_the_environment_and_bounded() {
    let read = |pairs: &'static [(&'static str, &'static str)]| {
        NodeConfig::from_lookup(move |key| match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some("/x".to_owned()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            other => pairs
                .iter()
                .find(|(k, _)| *k == other)
                .map(|(_, v)| (*v).to_owned()),
        })
    };
    let defaults = read(&[]).expect("valid").write_limit;
    assert_eq!(
        (
            defaults.target_millis,
            defaults.budget_millis,
            defaults.min_in_flight,
            defaults.max_in_flight
        ),
        (1_000, 5_000, 4, 1_024)
    );
    let set = read(&[
        ("OSS_WRITE_TARGET_MS", "200"),
        ("OSS_WRITE_BUDGET_MS", "900"),
        ("OSS_WRITE_MIN_IN_FLIGHT", "2"),
        ("OSS_WRITE_MAX_IN_FLIGHT", "64"),
    ])
    .expect("valid")
    .write_limit;
    assert_eq!(
        (
            set.target_millis,
            set.budget_millis,
            set.min_in_flight,
            set.max_in_flight
        ),
        (200, 900, 2, 64)
    );
    assert!(read(&[("OSS_WRITE_MAX_IN_FLIGHT", "0")]).is_err());
    assert!(read(&[("OSS_WRITE_TARGET_MS", "soon")]).is_err());
}
