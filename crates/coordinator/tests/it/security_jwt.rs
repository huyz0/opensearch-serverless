//! The claim rules of a token and the roles of a token identity
//! (docs/specs/security-complete/SPEC.md, slice 5; ADR 0090).

use oss_coordinator::security::{
    Config, Kind, Rules, Who, admissible, authinfo, claims, header_alg, securityconfig,
};
use oss_doc::mapping::json::parse;
use serde_json::{Value, json};

const NOW: u64 = 1_000_000;

fn rules() -> Rules {
    Rules {
        roles_key: Some("roles".to_owned()),
        issuer: Some("iss-a".to_owned()),
        audience: Some("aud-a".to_owned()),
        ..Rules::default()
    }
}

fn payload(extra: Value) -> Vec<u8> {
    let mut base =
        json!({"sub":"s","exp":NOW + 100,"iss":"iss-a","aud":"aud-a","roles":["r1","r2"]});
    for (k, v) in extra.as_object().expect("object") {
        if v.is_null() {
            base.as_object_mut().expect("object").remove(k);
        } else {
            base[k] = v.clone();
        }
    }
    base.to_string().into_bytes()
}

#[test]
fn a_good_payload_yields_its_subject_and_roles() {
    let got = claims(&payload(json!({})), &rules(), NOW).expect("accepted");
    assert_eq!(got.subject, "s");
    assert_eq!(got.roles, ["r1", "r2"]);
    let one = claims(
        &payload(json!({"roles":"solo","aud":["x","aud-a"]})),
        &rules(),
        NOW,
    );
    assert_eq!(one.expect("accepted").roles, ["solo"]);
    let none = claims(&payload(json!({"roles":null})), &rules(), NOW);
    assert!(none.expect("accepted").roles.is_empty());
}

#[test]
fn every_rule_refuses() {
    let refused = |extra: Value| claims(&payload(extra), &rules(), NOW).is_none();
    assert!(refused(json!({"exp":null})), "exp is required");
    assert!(refused(json!({"exp":"soon"})));
    assert!(refused(json!({"exp":NOW - 31})), "expired beyond the skew");
    assert!(!refused(json!({"exp":NOW - 29})), "inside the skew");
    assert!(refused(json!({"nbf":NOW + 31})));
    assert!(!refused(json!({"nbf":NOW + 29})));
    assert!(refused(json!({"iss":"iss-b"})));
    assert!(refused(json!({"iss":null})));
    assert!(refused(json!({"aud":"aud-b"})));
    assert!(refused(json!({"aud":null})));
    assert!(refused(json!({"sub":null})));
    assert!(refused(json!({"sub":""})));
    assert!(refused(json!({"sub":7})));
    assert!(refused(json!({"roles":[1]})));
    assert!(refused(json!({"roles":{"a":1}})));
    let many: Vec<String> = (0..65).map(|i| format!("r{i}")).collect();
    assert!(refused(json!({"roles": many})));
    assert!(refused(json!({"roles": ["x".repeat(257)]})));
    assert!(claims(b"[]", &rules(), NOW).is_none());
    assert!(claims(b"nope", &rules(), NOW).is_none());
}

#[test]
fn a_lifetime_over_the_bound_is_refused() {
    let bounded = Rules {
        max_lifetime: Some(60),
        ..rules()
    };
    assert!(claims(&payload(json!({"exp":NOW + 60})), &bounded, NOW).is_some());
    assert!(
        claims(&payload(json!({"exp":NOW + 61})), &bounded, NOW).is_some(),
        "skew"
    );
    assert!(claims(&payload(json!({"exp":NOW + 91})), &bounded, NOW).is_none());
}

#[test]
fn only_a_plain_algorithm_header_is_read() {
    assert_eq!(
        header_alg(br#"{"alg":"HS256","typ":"JWT","kid":"k"}"#).as_deref(),
        Some("HS256")
    );
    assert!(header_alg(br#"{"typ":"JWT"}"#).is_none());
    assert!(header_alg(br#"{"alg":5}"#).is_none());
    assert!(header_alg(br#"{"alg":"HS256","crit":["b64"]}"#).is_none());
    assert!(header_alg(b"[]").is_none());
}

fn config() -> Config {
    let mut c = Config::new().with_admin("admin", "$2b$04$adminhash");
    c.add(
        Kind::InternalUsers,
        "alice",
        &parse(br#"{"hash":"h","backend_roles":["analyst"],"opendistro_security_roles":["own"]}"#)
            .expect("json"),
    );
    for (name, doc) in [
        ("by_backend", r#"{"backend_roles":["analyst"]}"#),
        ("by_name", r#"{"users":["alice","jwt-user"]}"#),
        ("by_star", r#"{"users":["*"]}"#),
        ("by_all", r#"{"and_backend_roles":["analyst","ops"]}"#),
    ] {
        c.add(
            Kind::RolesMapping,
            name,
            &parse(doc.as_bytes()).expect("json"),
        );
    }
    c
}

#[test]
fn a_token_identity_takes_roles_only_from_its_backend_roles() {
    let c = config();
    let analyst = vec!["analyst".to_owned()];
    assert_eq!(c.roles_of(Who::token("jwt-user", &analyst)), ["by_backend"]);
    let both = vec!["analyst".to_owned(), "ops".to_owned()];
    assert_eq!(
        c.roles_of(Who::token("jwt-user", &both)),
        ["by_all", "by_backend"]
    );
    assert!(c.roles_of(Who::token("jwt-user", &[])).is_empty());
    // A stored user keeps its own roles and every mapping that names it.
    assert_eq!(
        c.roles_of(Who::stored("alice")),
        ["by_backend", "by_name", "by_star", "own"]
    );
    // A token that names a stored user takes nothing of the stored user.
    assert_eq!(c.roles_of(Who::token("alice", &[])), Vec::<String>::new());
    assert!(!c.is_admin(Who::token("admin", &[])));
    assert_eq!(
        c.backend_roles_of(Who::token("x", &analyst)),
        analyst.as_slice()
    );
}

#[test]
fn a_subject_naming_a_user_or_the_administrator_is_inadmissible() {
    let c = config();
    let claims = |s: &str| oss_coordinator::security::Claims {
        subject: s.to_owned(),
        roles: Vec::new(),
    };
    assert!(admissible(&c, &claims("jwt-user")));
    assert!(!admissible(&c, &claims("alice")));
    assert!(!admissible(&c, &claims("admin")));
}

#[test]
fn authinfo_of_a_token_identity_lists_its_roles() {
    let c = config();
    let analyst = vec!["analyst".to_owned()];
    let got: Value =
        serde_json::from_str(&authinfo(&c, Who::token("jwt-user", &analyst))).expect("json");
    assert_eq!(got["backend_roles"], json!(["analyst"]));
    assert_eq!(got["roles"], json!(["by_backend"]));
    assert_eq!(got["user_name"], "jwt-user");
}

#[test]
fn securityconfig_adds_the_domain_with_the_key_redacted() {
    let plain: Value = serde_json::from_str(&securityconfig(None)).expect("json");
    assert!(plain["config"]["dynamic"]["authc"]["jwt_auth_domain"].is_null());
    let with: Value = serde_json::from_str(&securityconfig(Some(&rules()))).expect("json");
    let authc = with["config"]["dynamic"]["authc"]
        .as_object()
        .expect("authc");
    assert_eq!(authc.len(), 2);
    let domain = &authc["jwt_auth_domain"]["http_authenticator"];
    assert_eq!(domain["type"], "jwt");
    assert_eq!(domain["config"]["roles_key"], "roles");
    assert_eq!(domain["config"]["subject_key"], "sub");
    assert_eq!(domain["config"]["signing_key"], "******");
    let none: Value = serde_json::from_str(&securityconfig(Some(&Rules::default()))).expect("json");
    assert!(none["config"]["dynamic"]["authc"]["jwt_auth_domain"]["http_authenticator"]["config"]["roles_key"].is_null());
}
