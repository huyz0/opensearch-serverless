//! The security settings' configuration tests.

use super::*;

// docs/specs/security-config AC-7
#[test]
fn security_is_off_by_default_and_set_by_its_variable() {
    assert!(!NodeConfig::from_lookup(lookup(FS)).expect("valid").security);
    let on = NodeConfig::from_lookup(lookup(&with(&[("OSS_SECURITY", "true")])));
    assert!(on.expect("valid").security);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_SECURITY", "yes")]))).is_err());
}

// docs/specs/security-authn AC-12: no flag is needed to turn security on
#[test]
fn configuring_an_administrator_turns_security_on_and_the_variable_still_wins() {
    let admin = [
        ("OSS_SECURITY_ADMIN_USER", "root"),
        ("OSS_SECURITY_ADMIN_HASH", "$2b$12$h"),
    ];
    assert!(
        NodeConfig::from_lookup(lookup(&with(&admin)))
            .expect("valid")
            .security
    );
    let off = [admin[0], admin[1], ("OSS_SECURITY", "false")];
    assert!(
        !NodeConfig::from_lookup(lookup(&with(&off)))
            .expect("valid")
            .security
    );
    // On with no administrator is still refused at assembly, never open.
    let on = [("OSS_SECURITY", "true")];
    assert!(
        NodeConfig::from_lookup(lookup(&with(&on)))
            .expect("valid")
            .security
    );
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

// docs/specs/ltr: the learning to rank store is a switch, on since it was measured
#[test]
fn learning_to_rank_is_on_by_default_and_set_by_its_variable() {
    assert!(NodeConfig::from_lookup(lookup(FS)).expect("valid").ltr);
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_LTR", "false")])));
    assert!(!off.expect("valid").ltr);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_LTR", "maybe")]))).is_err());
}

// docs/specs/ism: no plugin needs a flag to be on; the variable only turns it off
#[test]
fn index_state_management_is_on_by_default_and_set_by_its_variable() {
    assert!(
        NodeConfig::from_lookup(lookup(FS))
            .expect("valid")
            .plugins
            .ism
    );
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_ISM", "false")])));
    assert!(!off.expect("valid").plugins.ism);
}

// docs/specs/observability: the object routes are on by default and set by their variable
#[test]
fn observability_is_on_by_default_and_set_by_its_variable() {
    let on = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(on.plugins.observability);
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_OBSERVABILITY", "false")])));
    assert!(!off.expect("valid").plugins.observability);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_OBSERVABILITY", "maybe")]))).is_err());
}

// docs/specs/reports: the definition routes are on by default and set by their variable
#[test]
fn reports_are_on_by_default_and_set_by_their_variable() {
    let on = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(on.plugins.reports);
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_REPORTS", "false")])));
    assert!(!off.expect("valid").plugins.reports);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_REPORTS", "maybe")]))).is_err());
}

// docs/specs/anomaly-detection: no plugin needs a flag to be on; the variable only turns it off
#[test]
fn anomaly_detection_is_on_by_default_and_set_by_its_variable() {
    let on = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(on.plugins.ad);
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_AD", "false")])));
    assert!(!off.expect("valid").plugins.ad);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_AD", "maybe")]))).is_err());
}

// docs/specs/security-complete AC-21: the JSON Web Token domain is set by its variables
#[test]
fn a_jwt_key_sets_the_domain_and_a_bad_one_is_refused() {
    use crate::security_jwt::Key;
    let secret = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
    let none = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(none.security_jwt.is_none());
    let set = [
        ("OSS_SECURITY_JWT_KEY", secret),
        ("OSS_SECURITY_JWT_ROLES_KEY", "roles"),
        ("OSS_SECURITY_JWT_ISSUER", "me"),
        ("OSS_SECURITY_JWT_SKEW_SECONDS", "5"),
        ("OSS_SECURITY_JWT_MAX_LIFETIME_SECONDS", "600"),
    ];
    let jwt = NodeConfig::from_lookup(lookup(&with(&set)))
        .expect("valid")
        .security_jwt
        .expect("a domain");
    assert!(matches!(jwt.key, Key::Secret(ref k) if k.len() == 32));
    assert_eq!(jwt.rules.subject_key, "sub");
    assert_eq!(jwt.rules.roles_key.as_deref(), Some("roles"));
    assert_eq!(jwt.rules.issuer.as_deref(), Some("me"));
    assert_eq!((jwt.rules.skew, jwt.rules.max_lifetime), (5, Some(600)));
    for bad in [
        [("OSS_SECURITY_JWT_KEY", "c2hvcnQ=")],
        [("OSS_SECURITY_JWT_KEY", "!!!")],
        [("OSS_SECURITY_JWT_PUBLIC_KEY", "not a pem")],
    ] {
        assert!(
            NodeConfig::from_lookup(lookup(&with(&bad))).is_err(),
            "{bad:?}"
        );
    }
    let both = [
        ("OSS_SECURITY_JWT_KEY", secret),
        ("OSS_SECURITY_JWT_PUBLIC_KEY", "x"),
    ];
    assert!(NodeConfig::from_lookup(lookup(&with(&both))).is_err());
    let number = [
        ("OSS_SECURITY_JWT_KEY", secret),
        ("OSS_SECURITY_JWT_SKEW_SECONDS", "soon"),
    ];
    assert!(NodeConfig::from_lookup(lookup(&with(&number))).is_err());
}
