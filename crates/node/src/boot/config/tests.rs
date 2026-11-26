//! The node configuration's tests.
use super::*;
use oss_mem::Pool;

pub(super) fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

pub(super) const FS: &[(&str, &str)] = &[
    ("OSS_NODE_ID", "n1"),
    ("OSS_STORE", "fs"),
    ("OSS_FS_ROOT", "/data"),
];

pub(super) fn with<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut all: Vec<(&str, &str)> = FS.to_vec();
    all.extend_from_slice(extra);
    all
}

#[test]
fn the_lease_refresh_interval_is_set_apart_from_the_renewal() {
    let c =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_LEASE_REFRESH_MS", "2000")]))).expect("valid");
    assert_eq!(c.refresh_every(), std::time::Duration::from_secs(2));
    assert_eq!(
        c.renew_every(),
        std::time::Duration::from_secs(10),
        "the renewal stays"
    );
}

#[test]
fn a_file_store_node_takes_the_defaults() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(c.node_id, "n1");
    assert_eq!(c.cluster, "opensearch-serverless");
    assert_eq!(c.base, "");
    assert_eq!(c.http, "127.0.0.1:9200".parse().expect("addr"));
    assert_eq!(c.transport, "127.0.0.1:9300".parse().expect("addr"));
    assert_eq!(c.advertise, None);
    assert_eq!(c.memory_bytes, 1 << 30);
    assert_eq!(c.lease_ttl_millis, 30_000);
    assert_eq!(c.lease_refresh_millis, None);
    assert_eq!(c.cluster_settings_poll_millis, 30_000);
    assert_eq!(c.refresh_every(), c.renew_every(), "as often as renewed");
    assert_eq!(c.index_confirm_millis, 5_000);
    assert_eq!(c.janitor_every_millis, 60_000);
    assert_eq!(c.renew_every(), std::time::Duration::from_secs(10));
    assert_eq!(
        c.store,
        StoreChoice::Fs {
            root: "/data".into()
        }
    );
}

#[test]
fn proactive_takeover_is_on_with_the_settings_defaults_and_each_is_read() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(c.proactive_takeover);
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_PROACTIVE_TAKEOVER", "false")])))
        .expect("valid");
    assert!(!off.proactive_takeover);
    assert_eq!(
        (
            c.proactive_reapers,
            c.proactive_max_concurrent,
            c.proactive_pass_millis
        ),
        (2, 16, 60_000)
    );
    let on = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_PROACTIVE_TAKEOVER", "true"),
        ("OSS_PROACTIVE_REAPERS", "3"),
        ("OSS_PROACTIVE_MAX_CONCURRENT", "8"),
        ("OSS_PROACTIVE_PASS_MS", "5000"),
    ])))
    .expect("valid");
    assert!(on.proactive_takeover);
    assert_eq!(
        (
            on.proactive_reapers,
            on.proactive_max_concurrent,
            on.proactive_pass_millis
        ),
        (3, 8, 5_000)
    );
}

#[test]
fn a_proactive_setting_out_of_its_bounds_is_refused() {
    for (key, bad) in [
        ("OSS_PROACTIVE_TAKEOVER", "yes"),
        ("OSS_PROACTIVE_REAPERS", "0"),
        ("OSS_PROACTIVE_REAPERS", "17"),
        ("OSS_PROACTIVE_MAX_CONCURRENT", "0"),
        ("OSS_PROACTIVE_MAX_CONCURRENT", "65"),
        ("OSS_PROACTIVE_PASS_MS", "0"),
        ("OSS_PROACTIVE_PASS_MS", "3600001"),
        ("OSS_PROACTIVE_PASS_MS", "x"),
    ] {
        let err = NodeConfig::from_lookup(lookup(&with(&[(key, bad)]))).expect_err(key);
        assert!(err.contains(key), "{err}");
    }
}

#[test]
fn an_s3_node_names_its_bucket() {
    let pairs = [
        ("OSS_NODE_ID", "n1"),
        ("OSS_STORE", "s3"),
        ("OSS_S3_ENDPOINT", "http://127.0.0.1:9000"),
        ("OSS_S3_BUCKET", "b"),
        ("OSS_S3_REGION", "r"),
        ("OSS_S3_ACCESS_KEY", "ak"),
        ("OSS_S3_SECRET_KEY", "sk"),
        ("OSS_BASE", "prod/"),
    ];
    let c = NodeConfig::from_lookup(lookup(&pairs)).expect("valid");
    assert_eq!(c.base, "prod/");
    assert!(matches!(c.store, StoreChoice::S3 { ref bucket, .. } if bucket == "b"));
}

#[test]
fn what_cannot_be_used_is_refused_by_name() {
    let refused = |pairs: &[(&str, &str)], needle: &str| {
        let e = NodeConfig::from_lookup(lookup(pairs)).expect_err("refused");
        assert!(e.contains(needle), "{e} lacks {needle}");
    };
    refused(&[], "OSS_NODE_ID");
    refused(&[("OSS_NODE_ID", "n")], "OSS_STORE");
    refused(&[("OSS_NODE_ID", "n"), ("OSS_STORE", "gcs")], "[gcs]");
    refused(&[("OSS_NODE_ID", "n"), ("OSS_STORE", "fs")], "OSS_FS_ROOT");
    refused(
        &[("OSS_NODE_ID", "n"), ("OSS_STORE", "s3")],
        "OSS_S3_ENDPOINT",
    );
    refused(&with(&[("OSS_BASE", "prod")]), "OSS_BASE");
    refused(&with(&[("OSS_HTTP_ADDR", "nope")]), "OSS_HTTP_ADDR");
    refused(
        &with(&[("OSS_TRANSPORT_ADDR", "nope")]),
        "OSS_TRANSPORT_ADDR",
    );
    refused(&with(&[("OSS_MEMORY_BYTES", "x")]), "OSS_MEMORY_BYTES");
    refused(&with(&[("OSS_MEMORY_BYTES", "1000")]), "OSS_MEMORY_BYTES");
    refused(&with(&[("OSS_LEASE_TTL_MS", "y")]), "OSS_LEASE_TTL_MS");
    refused(
        &with(&[("OSS_LEASE_REFRESH_MS", "y")]),
        "OSS_LEASE_REFRESH_MS",
    );
    refused(
        &with(&[("OSS_LEASE_REFRESH_MS", "0")]),
        "OSS_LEASE_REFRESH_MS",
    );
    refused(
        &with(&[("OSS_CLUSTER_SETTINGS_POLL_MS", "0")]),
        "OSS_CLUSTER_SETTINGS_POLL_MS",
    );
    refused(
        &with(&[("OSS_CLUSTER_SETTINGS_POLL_MS", "soon")]),
        "OSS_CLUSTER_SETTINGS_POLL_MS",
    );
    refused(&with(&[("OSS_LEASE_TTL_MS", "2")]), "OSS_LEASE_TTL_MS");
    refused(
        &with(&[("OSS_JANITOR_EVERY_MS", "0")]),
        "OSS_JANITOR_EVERY_MS",
    );
    refused(
        &with(&[("OSS_JANITOR_EVERY_MS", "x")]),
        "OSS_JANITOR_EVERY_MS",
    );
    refused(&with(&[("OSS_TAIL_OVERLAY", "yes")]), "OSS_TAIL_OVERLAY");
    refused(
        &with(&[("OSS_REFRESH_VISIBILITY", "all")]),
        "OSS_REFRESH_VISIBILITY",
    );
}

#[test]
fn the_tail_overlay_is_on_and_visibility_everywhere_unless_set() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(c.tail_overlay);
    let off =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_TAIL_OVERLAY", "false")]))).expect("valid");
    assert!(!off.tail_overlay);
    assert_eq!(c.refresh_visibility, "everywhere");
    let mut pairs = FS.to_vec();
    pairs.push(("OSS_TAIL_OVERLAY", "true"));
    pairs.push(("OSS_REFRESH_VISIBILITY", "serving"));
    let c = NodeConfig::from_lookup(lookup(&pairs)).expect("valid");
    assert!(c.tail_overlay);
    assert_eq!(c.refresh_visibility, "serving");
}

#[test]
fn the_secret_is_not_printed() {
    let pairs = [
        ("OSS_NODE_ID", "n1"),
        ("OSS_STORE", "s3"),
        ("OSS_S3_ENDPOINT", "http://e"),
        ("OSS_S3_BUCKET", "b"),
        ("OSS_S3_REGION", "r"),
        ("OSS_S3_ACCESS_KEY", "ak"),
        ("OSS_S3_SECRET_KEY", "hunter2"),
    ];
    let c = NodeConfig::from_lookup(lookup(&pairs)).expect("valid");
    let shown = format!("{c:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
    assert!(shown.contains("ak"));
}

#[test]
fn the_pools_fit_and_every_pool_is_given_bytes() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    let b = c.budget().expect("fits");
    let sum: usize = Pool::ALL.iter().map(|p| b.pool(*p)).sum();
    assert!(sum <= c.memory_bytes);
    assert!(
        Pool::ALL
            .iter()
            .filter(|p| **p != Pool::Extensions)
            .all(|p| b.pool(*p) > 0)
    );
    assert_eq!(
        b.pool(Pool::Extensions),
        0,
        "no extensions pool unless asked"
    );
    assert_eq!(b.pool(Pool::OpenIndexes), c.memory_bytes / 100 * 20);
}

#[test]
fn the_janitor_deletes_unless_the_variable_makes_it_a_dry_run() {
    let live = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(!live.janitor_dry_run);
    let dry =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_JANITOR_DRY_RUN", "true")]))).expect("valid");
    assert!(dry.janitor_dry_run);
}

#[test]
fn the_buckets_switch_is_off_by_default_and_reads_a_flag() {
    assert!(
        !NodeConfig::from_lookup(lookup(FS))
            .expect("valid")
            .buckets_enabled
    );
    let on =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_BUCKETS_ENABLED", "true")]))).expect("valid");
    assert!(on.buckets_enabled);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_BUCKETS_ENABLED", "yes")]))).is_err());
}

#[test]
fn plugin_upstreams_are_off_by_default_and_listed_by_commas() {
    let off = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert!(off.plugin_upstreams.is_empty());
    assert_eq!(
        (off.plugin_max_request_bytes, off.plugin_timeout_millis),
        (10 << 20, 30_000)
    );
    let on = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_PLUGIN_UPSTREAMS", " http://a:9200, https://b:9200 ,,"),
        ("OSS_PLUGIN_MAX_REQUEST_BYTES", "2048"),
        ("OSS_PLUGIN_TIMEOUT_MS", "500"),
    ])))
    .expect("valid");
    assert_eq!(on.plugin_upstreams, ["http://a:9200", "https://b:9200"]);
    assert_eq!(
        (on.plugin_max_request_bytes, on.plugin_timeout_millis),
        (2048, 500)
    );
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_PLUGIN_TIMEOUT_MS", "0")]))).is_err());
}

#[test]
fn the_give_back_threshold_defaults_to_never_and_is_set_by_the_variable() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(c.give_back_after_millis, 0);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_GIVE_BACK_AFTER_MS", "300000")])))
        .expect("valid");
    assert_eq!(c.give_back_after_millis, 300_000);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_GIVE_BACK_AFTER_MS", "soon")]))).is_err());
}

#[test]
fn a_give_back_threshold_beyond_the_day_a_record_is_kept_is_refused() {
    assert!(
        NodeConfig::from_lookup(lookup(&with(&[("OSS_GIVE_BACK_AFTER_MS", "86400001")]))).is_err()
    );
    assert!(
        NodeConfig::from_lookup(lookup(&with(&[("OSS_GIVE_BACK_AFTER_MS", "86400000")]))).is_ok()
    );
}

#[test]
fn the_commit_push_is_on_with_the_overlay_off_and_off_with_it_on_unless_the_variable_says() {
    let overlay = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(
        !overlay.commit_push,
        "the overlay is on and serves freshness"
    );
    let no_overlay =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_TAIL_OVERLAY", "false")]))).expect("valid");
    assert!(no_overlay.commit_push);
    let both = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_TAIL_OVERLAY", "true"),
        ("OSS_COMMIT_PUSH", "true"),
    ])))
    .expect("valid");
    assert!(both.commit_push);
    let off =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_COMMIT_PUSH", "false")]))).expect("valid");
    assert!(!off.commit_push);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_COMMIT_PUSH", "maybe")]))).is_err());
}

// docs/specs/writer-shard-cap AC-7
#[test]
fn the_shard_cap_is_off_by_default_and_set_by_its_variables() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!((c.max_shards, c.evict_after_millis), (0, 60_000));
    let c = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_MAX_SHARDS", "500"),
        ("OSS_EVICT_AFTER_MS", "0"),
    ])))
    .expect("valid");
    assert_eq!((c.max_shards, c.evict_after_millis), (500, 0));
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_MAX_SHARDS", "many")]))).is_err());
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_EVICT_AFTER_MS", "soon")]))).is_err());
}

// docs/specs/writer-headroom-pins AC-7
#[test]
fn headroom_and_pins_are_off_by_default_and_set_by_their_variables() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(c.evict_headroom_percent, 0);
    assert!(c.pinned_indexes.is_empty());
    let c = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_EVICT_HEADROOM_PCT", "5"),
        ("OSS_PINNED_INDEXES", " a, b ,,c"),
    ])))
    .expect("valid");
    assert_eq!(c.evict_headroom_percent, 5);
    assert_eq!(c.pinned_indexes, ["a", "b", "c"]);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_EVICT_HEADROOM_PCT", "101")]))).is_err());
}

// docs/specs/knn-vector-index AC-4
#[test]
fn knn_is_on_by_default_and_turned_off_by_its_variable() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(c.knn);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_KNN", "false")]))).expect("valid");
    assert!(!c.knn);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_KNN", "true")]))).expect("valid");
    assert!(c.knn);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_KNN", "yes")]))).is_err());
}

// docs/specs/writer-hand-off AC-8
#[test]
fn hand_off_is_off_by_default_and_set_by_its_variable() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(!c.hand_off);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_HAND_OFF", "true")]))).expect("valid");
    assert!(c.hand_off);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_HAND_OFF", "yes")]))).is_err());
}

#[test]
fn a_node_with_the_switch_off_has_no_bucket_wiring() {
    use oss_blob::{BlobStore, Emulator};
    use oss_cluster::{LeaseClock, SystemClock};
    use oss_meta::{DeleteMode, DescriptorStore};
    use std::sync::Arc;

    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let descriptors = |c: &NodeConfig| {
        Arc::new(
            DescriptorStore::new(Arc::clone(&store), &c.base, DeleteMode::Tombstone)
                .expect("store"),
        )
    };
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(
        crate::boot::support::bucket_wiring(&off, &store, &descriptors(&off), &clock)
            .expect("builds")
            .is_none()
    );
    let on =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_BUCKETS_ENABLED", "true")]))).expect("valid");
    assert!(
        crate::boot::support::bucket_wiring(&on, &store, &descriptors(&on), &clock)
            .expect("builds")
            .is_some()
    );
}

// docs/specs/store-usage-wiring AC-1
#[test]
fn store_usage_is_on_by_default_off_when_set_and_bounded() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(c.store_usage);
    assert_eq!(c.store_usage_max_indices, 1000);
    let off = with(&[("OSS_STORE_USAGE", "false")]);
    let c = NodeConfig::from_lookup(lookup(&off)).expect("valid");
    assert!(!c.store_usage);
    let set = with(&[("OSS_STORE_USAGE_MAX_INDICES", "7")]);
    let c = NodeConfig::from_lookup(lookup(&set)).expect("valid");
    assert!(c.store_usage);
    assert_eq!(c.store_usage_max_indices, 7);
    for bad in ["0", "1000001"] {
        let set = with(&[("OSS_STORE_USAGE_MAX_INDICES", bad)]);
        assert!(NodeConfig::from_lookup(lookup(&set)).is_err());
    }
}

// docs/specs/tenant-levels-wiring AC-6
#[test]
fn tenant_levels_observe_with_a_large_budget_unless_set() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(c.tenant_levels, "observe");
    assert_eq!(c.tenant_budget_per_minute, 2_000);
    let c = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_TENANT_LEVELS", "enforce"),
        ("OSS_TENANT_BUDGET_PER_MINUTE", "50"),
    ])))
    .expect("valid");
    assert_eq!(c.tenant_levels, "enforce");
    assert_eq!(c.tenant_budget_per_minute, 50);
    for (pairs, needle) in [
        (with(&[("OSS_TENANT_LEVELS", "maybe")]), "OSS_TENANT_LEVELS"),
        (
            with(&[("OSS_TENANT_BUDGET_PER_MINUTE", "0")]),
            "OSS_TENANT_BUDGET_PER_MINUTE",
        ),
    ] {
        let e = NodeConfig::from_lookup(lookup(&pairs)).expect_err("refused");
        assert!(e.contains(needle), "{e} lacks {needle}");
    }
}

// docs/specs/tenant-tier-budgets AC-6
#[test]
fn tenant_tier_budgets_default_to_twenty_two_hundred_and_two_thousand() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(
        (
            c.tenant_budget_small,
            c.tenant_budget_standard,
            c.tenant_budget_per_minute
        ),
        (20, 200, 2_000)
    );
    assert_eq!(c.tenant_default_tier, "");
    let c = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_TENANT_BUDGET_SMALL", "5"),
        ("OSS_TENANT_BUDGET_STANDARD", "50"),
        ("OSS_TENANT_DEFAULT_TIER", "small"),
    ])))
    .expect("valid");
    assert_eq!(
        (
            c.tenant_budget_small,
            c.tenant_budget_standard,
            c.tenant_default_tier
        ),
        (5, 50, "small")
    );
    for (pairs, needle) in [
        (
            with(&[("OSS_TENANT_DEFAULT_TIER", "huge")]),
            "OSS_TENANT_DEFAULT_TIER",
        ),
        (
            with(&[("OSS_TENANT_BUDGET_SMALL", "0")]),
            "OSS_TENANT_BUDGET_SMALL",
        ),
        (
            with(&[("OSS_TENANT_BUDGET_STANDARD", "0")]),
            "OSS_TENANT_BUDGET_STANDARD",
        ),
    ] {
        let e = NodeConfig::from_lookup(lookup(&pairs)).expect_err("refused");
        assert!(e.contains(needle), "{e} lacks {needle}");
    }
}
mod extensions;
mod proactive;
mod resident;
mod security;
mod vector_warm;
