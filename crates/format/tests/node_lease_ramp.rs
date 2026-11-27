//! A lease's `search_since` and `ramp_permille` (docs/specs/reader-slow-start/SPEC.md, AC-2).

#![allow(clippy::disallowed_methods)]

use oss_format::{MAX_LEASE_BYTES, NodeLease};

fn written(lease: &NodeLease) -> String {
    let mut buf = vec![0u8; MAX_LEASE_BYTES];
    let n = lease.write(&mut buf).unwrap();
    buf.truncate(n);
    String::from_utf8(buf).unwrap()
}

#[test]
fn a_lease_without_the_fields_is_written_as_before() {
    let lease = NodeLease::new("n1", "e1", "10.0.0.1:9300", 5);
    let text = written(&lease);
    assert!(
        !text.contains("search_since") && !text.contains("ramp_permille"),
        "{text}"
    );
}

#[test]
fn the_fields_round_trip_after_the_zone_and_before_the_log() {
    let mut lease = NodeLease::new("n1", "e1", "a", 5);
    lease.search_since = Some(1_700_000_000_000);
    lease.set_ramp_permille(Some(300));
    let text = written(&lease);
    assert!(
        text.ends_with(r#","search_since":1700000000000,"ramp_permille":300}"#),
        "{text}"
    );
    let back = NodeLease::parse(text.as_bytes()).unwrap();
    assert_eq!(back.search_since, Some(1_700_000_000_000));
    assert_eq!(back.ramp_permille(), Some(300));
    assert_eq!(back, lease);
}

#[test]
fn a_ramp_outside_the_range_is_clamped_on_read_and_on_set() {
    let text = r#"{"node_id":"n","ephemeral_id":"e","address":"a","roles":[],"expires_at_millis":1,"ramp_permille":5000}"#;
    assert_eq!(
        NodeLease::parse(text.as_bytes()).unwrap().ramp_permille(),
        Some(1000)
    );
    let text = text.replace("5000", "-4");
    assert_eq!(
        NodeLease::parse(text.as_bytes()).unwrap().ramp_permille(),
        Some(0)
    );
    let mut lease = NodeLease::new("n", "e", "a", 1);
    lease.set_ramp_permille(Some(-1));
    assert_eq!(lease.ramp_permille(), Some(0));
}

#[test]
fn a_repeated_or_mistyped_field_is_refused() {
    let base =
        r#"{"node_id":"n","ephemeral_id":"e","address":"a","roles":[],"expires_at_millis":1,"#;
    for tail in [
        r#""search_since":1,"search_since":2}"#,
        r#""ramp_permille":"1"}"#,
        r#""search_since":1.5}"#,
    ] {
        assert!(
            NodeLease::parse(format!("{base}{tail}").as_bytes()).is_err(),
            "{tail}"
        );
    }
}
