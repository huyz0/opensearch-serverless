//! The security settings' configuration tests.

use super::tests::{FS, lookup, with};
use super::*;

// docs/specs/security-config AC-7
#[test]
fn security_is_off_by_default_and_set_by_its_variable() {
    assert!(!NodeConfig::from_lookup(lookup(FS)).expect("valid").security);
    let on = NodeConfig::from_lookup(lookup(&with(&[("OSS_SECURITY", "true")])));
    assert!(on.expect("valid").security);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_SECURITY", "yes")]))).is_err());
}

// docs/specs/security-authn AC-9
#[test]
fn the_administrator_is_a_pair_and_the_refresh_is_bounded() {
    let c = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_SECURITY_ADMIN_USER", "root"),
        ("OSS_SECURITY_ADMIN_HASH", "$2b$12$h"),
    ])))
    .expect("valid");
    assert_eq!(
        c.security_admin,
        Some(("root".to_owned(), "$2b$12$h".to_owned()))
    );
    assert_eq!(c.security_refresh_millis, 10_000);
    assert!(
        NodeConfig::from_lookup(lookup(&with(&[("OSS_SECURITY_ADMIN_USER", "root")]))).is_err()
    );
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_SECURITY_REFRESH_MS", "999")]))).is_err());
    assert_eq!(
        NodeConfig::from_lookup(lookup(FS))
            .expect("valid")
            .security_admin,
        None
    );
}
