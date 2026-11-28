//! docs/specs/alerting/SPEC.md, slice 6: the alert register's key and bound.

use oss_format::{FormatError, MAX_ALERT_SET_BYTES, alert_set_key, check_alert_set};

#[test]
fn the_key_is_derived_from_the_base_tenant_and_monitor() {
    assert_eq!(
        alert_set_key("", "acme", "m1").unwrap(),
        "alerting/acme/alerts/m1"
    );
    assert_eq!(
        alert_set_key("b/", "acme", "m1").unwrap(),
        "b/alerting/acme/alerts/m1"
    );
}

#[test]
fn a_key_with_a_bad_part_is_refused() {
    assert!(alert_set_key("b", "acme", "m1").is_err());
    assert!(alert_set_key("", "a/b", "m1").is_err());
    assert!(alert_set_key("", "acme", "").is_err());
    assert!(alert_set_key("", "acme", "../x").is_err());
    assert!(alert_set_key("", "acme", "a\"b").is_err());
}

#[test]
fn a_set_is_utf8_within_its_bound() {
    check_alert_set(br#"{"alerts":[]}"#).unwrap();
    check_alert_set(&vec![b'a'; MAX_ALERT_SET_BYTES]).unwrap();
    assert!(matches!(
        check_alert_set(&vec![b'a'; MAX_ALERT_SET_BYTES + 1]),
        Err(FormatError::TooLarge { .. })
    ));
    assert!(matches!(
        check_alert_set(&[0xff, 0xfe]),
        Err(FormatError::Corrupt { .. })
    ));
}
