//! docs/specs/resident-pages: the resident page bound's setting.

use super::*;

// docs/specs/resident-pages AC-1
#[test]
fn the_resident_page_bound_is_off_by_default_and_refuses_what_it_cannot_hold() {
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(off.resident_page_bytes, 0);
    let set = NodeConfig::from_lookup(lookup(&with(&[("OSS_RESIDENT_PAGE_BYTES", "1048576")])))
        .expect("valid");
    assert_eq!(set.resident_page_bytes, 1 << 20);
    for bad in ["abc", "1", "1048575", "-5", "1125899906842625"] {
        let err = NodeConfig::from_lookup(lookup(&with(&[("OSS_RESIDENT_PAGE_BYTES", bad)])))
            .expect_err(bad);
        assert!(err.contains("OSS_RESIDENT_PAGE_BYTES"), "{bad}: {err}");
    }
}

// AC-8
#[test]
fn the_queue_is_off_by_default_and_takes_its_settings() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(
        c.search_queue,
        crate::search_queue::Config {
            size: 0,
            share_pct: 25,
            reserved: 8,
            wait_millis: 30_000,
            max_connections: 1024
        }
    );
    let on = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_SEARCH_QUEUE_SIZE", "500"),
        ("OSS_SEARCH_TENANT_SHARE_PCT", "10"),
        ("OSS_SEARCH_RESERVED_WORKERS", "16"),
        ("OSS_SEARCH_QUEUE_WAIT_MS", "5000"),
        ("OSS_HTTP_MAX_CONNECTIONS", "2048"),
    ])))
    .expect("valid");
    assert_eq!(
        (
            on.search_queue.size,
            on.search_queue.share_pct,
            on.search_queue.reserved
        ),
        (500, 10, 16)
    );
    assert_eq!(
        (on.search_queue.wait_millis, on.search_queue.max_connections),
        (5000, 2048)
    );
}

// AC-8
#[test]
fn the_queue_refuses_what_the_server_cannot_hold() {
    let refuse = |extra: &[(&str, &str)], names: &str| {
        let err = NodeConfig::from_lookup(lookup(&with(extra))).expect_err(names);
        assert!(err.contains(names), "{err}");
    };
    // 1,024 connections less 128 workers leave 896 to wait.
    refuse(&[("OSS_SEARCH_QUEUE_SIZE", "897")], "OSS_SEARCH_QUEUE_SIZE");
    refuse(&[("OSS_SEARCH_QUEUE_SIZE", "abc")], "OSS_SEARCH_QUEUE_SIZE");
    refuse(
        &[("OSS_HTTP_MAX_CONNECTIONS", "64")],
        "OSS_HTTP_MAX_CONNECTIONS",
    );
    let on = ("OSS_SEARCH_QUEUE_SIZE", "10");
    refuse(
        &[on, ("OSS_SEARCH_TENANT_SHARE_PCT", "0")],
        "OSS_SEARCH_TENANT_SHARE_PCT",
    );
    refuse(
        &[on, ("OSS_SEARCH_TENANT_SHARE_PCT", "101")],
        "OSS_SEARCH_TENANT_SHARE_PCT",
    );
    refuse(
        &[on, ("OSS_SEARCH_RESERVED_WORKERS", "128")],
        "OSS_SEARCH_RESERVED_WORKERS",
    );
    refuse(
        &[on, ("OSS_SEARCH_QUEUE_WAIT_MS", "0")],
        "OSS_SEARCH_QUEUE_WAIT_MS",
    );
    refuse(&[on, ("OSS_SEARCH_SLOTS", "8")], "OSS_SEARCH_SLOTS");
    // Raising the connections raises what the queue may hold.
    NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_SEARCH_QUEUE_SIZE", "2000"),
        ("OSS_HTTP_MAX_CONNECTIONS", "4096"),
    ])))
    .expect("fits");
}

// docs/specs/stored-script-ids AC-9
#[test]
fn the_stored_script_ttl_defaults_to_ten_seconds_and_is_bounded() {
    let default = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(default.script_cache_ttl_secs, 10);
    let set =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_SCRIPT_CACHE_TTL", "3600")]))).expect("valid");
    assert_eq!(set.script_cache_ttl_secs, 3_600);
    for bad in ["0", "3601", "abc", "-1"] {
        let err = NodeConfig::from_lookup(lookup(&with(&[("OSS_SCRIPT_CACHE_TTL", bad)])))
            .expect_err(bad);
        assert!(err.contains("OSS_SCRIPT_CACHE_TTL"), "{bad}: {err}");
    }
}
