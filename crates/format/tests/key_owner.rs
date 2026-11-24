//! The index a blob key names (docs/specs/store-usage/SPEC.md, AC-7).

use oss_format::index_of_key;

// AC-7
#[test]
fn keys_that_name_an_index_give_it() {
    for (key, index) in [
        ("indices/logs", "logs"),
        ("tombstones/2026112410/logs", "logs"),
        ("reclaim/29000000/logs#u1", "logs"),
        ("segments/logs#u1#3/manifest", "logs"),
        ("segments/logs#u1#3/t=2/seg-1", "logs"),
        ("shards/ab-logs#3", "logs"),
    ] {
        assert_eq!(index_of_key("", key), Some(index), "{key}");
        let based = format!("deploy/{key}");
        assert_eq!(index_of_key("deploy/", &based), Some(index), "{based}");
    }
}

// AC-7
#[test]
fn keys_no_single_index_owns_give_none() {
    for key in [
        "packs/n1/e=00000000000000000001/00000000000000000002",
        "node-log/n1/1",
        "leases/n1",
        "index-changes/stripe-01",
        "cluster-config",
        "indices/",
        "segments//manifest",
        "shards/ab-#3",
        "indices",
        "",
    ] {
        assert_eq!(index_of_key("", key), None, "{key}");
    }
    assert_eq!(index_of_key("deploy/", "other/indices/logs"), None);
}
