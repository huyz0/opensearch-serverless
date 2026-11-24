//! docs/specs/node-publish-age: how an index's `refresh_interval` becomes the
//! age its writes may reach before they are published.

#![cfg(feature = "writer")]

use oss_node::writer::publish_age_of;
use oss_writer::publish::lazy::PublishAge;

#[test]
fn a_refresh_interval_is_read_flat_nested_or_not_at_all() {
    for (settings, want) in [
        (
            &br#"{"index":{"refresh_interval":"5s"}}"#[..],
            PublishAge::Millis(5_000),
        ),
        (
            &br#"{"index.refresh_interval":"250ms"}"#[..],
            PublishAge::Millis(250),
        ),
        (
            &br#"{"index.refresh_interval":"1m"}"#[..],
            PublishAge::Millis(60_000),
        ),
        (
            &br#"{"index":{"refresh_interval":"0"}}"#[..],
            PublishAge::Millis(0),
        ),
        (
            &br#"{"index.refresh_interval":"-1"}"#[..],
            PublishAge::Never,
        ),
        (
            &br#"{"index":{"refresh_interval":-1}}"#[..],
            PublishAge::Never,
        ),
        (&b"{}"[..], PublishAge::Node),
        (
            &br#"{"index":{"number_of_shards":"2"}}"#[..],
            PublishAge::Node,
        ),
        (
            &br#"{"index.refresh_interval":"soon"}"#[..],
            PublishAge::Node,
        ),
        (&br#"{"index.refresh_interval":true}"#[..], PublishAge::Node),
        (&b"not json"[..], PublishAge::Node),
        (&b""[..], PublishAge::Node),
    ] {
        assert_eq!(
            publish_age_of(settings),
            want,
            "{}",
            String::from_utf8_lossy(settings)
        );
    }
}
