//! Whether an index is grouped into a bucket, and with what.

use oss_types::settings::{Empty, Layers, Registry, Source};
use oss_writer::bucket::policy::{
    CLUSTER_POLICY, GROUP, INDEX_POLICY, SETTINGS, check, group, wanted,
};

fn with(pairs: &'static [(&'static str, &'static str)]) -> impl Source {
    pairs
}

fn layers<'a>(
    index: Option<&'a dyn Source>,
    cluster: &'a dyn Source,
    node: &'a dyn Source,
) -> Layers<'a> {
    Layers {
        index,
        cluster,
        node,
    }
}

#[test]
fn unset_everywhere_the_node_decides() {
    assert!(!wanted(&layers(None, &Empty, &Empty)).unwrap());
    let on = with(&[("serverless.buckets.enabled", "true")]);
    assert!(wanted(&layers(None, &Empty, &on)).unwrap());
    assert!(wanted(&layers(Some(&Empty), &Empty, &on)).unwrap());
}

#[test]
fn the_index_beats_the_cluster_and_the_cluster_the_node() {
    let on = with(&[("serverless.buckets.enabled", "true")]);
    let auto = with(&[("serverless.bucket", "auto")]);
    let off = with(&[("serverless.bucket", "off")]);
    assert!(wanted(&layers(None, &auto, &Empty)).unwrap());
    assert!(!wanted(&layers(None, &off, &on)).unwrap());
    let index_off = with(&[("index.serverless.bucket", "OFF")]);
    let index_auto = with(&[("index.serverless.bucket", "Auto")]);
    assert!(!wanted(&layers(Some(&index_off), &auto, &on)).unwrap());
    assert!(wanted(&layers(Some(&index_auto), &off, &Empty)).unwrap());
}

#[test]
fn an_empty_or_unparsable_value_is_skipped_to_the_next_layer() {
    let off = with(&[("serverless.bucket", "off")]);
    let empty_index = with(&[("index.serverless.bucket", "")]);
    let bad_index = with(&[("index.serverless.bucket", "maybe")]);
    assert!(!wanted(&layers(Some(&empty_index), &off, &Empty)).unwrap());
    assert!(!wanted(&layers(Some(&bad_index), &off, &Empty)).unwrap());
    let bad_cluster = with(&[("serverless.bucket", "maybe")]);
    let on = with(&[("serverless.buckets.enabled", "true")]);
    assert!(wanted(&layers(None, &bad_cluster, &on)).unwrap());
    let empty_cluster = with(&[("serverless.bucket", "")]);
    assert!(wanted(&layers(None, &empty_cluster, &on)).unwrap());
}

#[test]
fn a_node_value_that_is_no_boolean_is_refused() {
    let bad = with(&[("serverless.buckets.enabled", "yes")]);
    assert!(wanted(&layers(None, &Empty, &bad)).is_err());
    // an index or cluster value decides before the node is read
    let off = with(&[("serverless.bucket", "off")]);
    assert!(!wanted(&layers(None, &off, &bad)).unwrap());
}

#[test]
fn check_takes_auto_off_and_unset() {
    for ok in ["auto", "off", "AUTO", "Off", ""] {
        assert!(check(ok).is_ok(), "{ok:?}");
    }
    for bad in ["on", "true", " auto", "auto ", "none"] {
        assert!(check(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_group_is_the_setting_or_the_index_name() {
    assert_eq!(group("logs-1", None), "logs-1");
    assert_eq!(group("logs-1", Some(&Empty)), "logs-1");
    let empty = with(&[("index.serverless.bucket_group", "")]);
    assert_eq!(group("logs-1", Some(&empty)), "logs-1");
    let named = with(&[("index.serverless.bucket_group", "tenant-7")]);
    assert_eq!(group("logs-1", Some(&named)), "tenant-7");
}

#[test]
fn the_settings_form_a_valid_registry() {
    assert!(Registry::new(&[SETTINGS]).is_ok());
    assert_eq!(SETTINGS.len(), 3);
    assert_eq!(INDEX_POLICY.key(), "index.serverless.bucket");
    assert_eq!(CLUSTER_POLICY.key(), "serverless.bucket");
    assert_eq!(GROUP.key(), "index.serverless.bucket_group");
}
