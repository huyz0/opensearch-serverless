//! JSON Web Token authentication (docs/specs/security-complete/SPEC.md,
//! slice 5, ADR 0090). The tokens are minted by `fixtures/security-jwt/mint.py`
//! with PyJWT, not by the code under test; every valid one expires in 2100
//! and the rig's wall clock is fixed in 2027.

use crate::security_authn::{ADMIN, API, Rig, call, make_user, rig_with};

use oss_blob::S3Op;
use oss_blob::{BlobStore, Emulator};
use oss_coordinator::security::Rules;
use oss_node::security_jwt::Key;
use oss_node::security_setting::JwtSettings;
use serde_json::Value;
use std::sync::Arc;

const TOKENS: &str = include_str!("../../../../fixtures/security-jwt/tokens.json");

fn tokens() -> Value {
    serde_json::from_str(TOKENS).expect("tokens.json")
}

fn rules() -> Rules {
    Rules {
        roles_key: Some("roles".to_owned()),
        issuer: Some("issuer-a".to_owned()),
        audience: Some("aud-a".to_owned()),
        ..Rules::default()
    }
}

fn rig_for(key: Key) -> Rig {
    rig_with(
        Arc::new(Emulator::new(1 << 28, 5)),
        Some(JwtSettings {
            key,
            rules: rules(),
        }),
    )
}

fn text(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn secret_rig() -> Rig {
    rig_for(Key::secret(text(&tokens()["hs_secret"])).expect("secret"))
}

fn bearer(r: &Rig, token: &str, path: &str) -> (u16, String) {
    call(r, Some(&format!("raw:Bearer {token}")), "GET", path, None)
}

const AUTHINFO: &str = "/_plugins/_security/authinfo";

#[test]
fn each_key_type_authenticates_its_tokens() {
    let t = tokens();
    let cases: [(Key, &[&str]); 4] = [
        (
            Key::secret(text(&t["hs_secret"])).expect("secret"),
            &["HS256", "HS384", "HS512"],
        ),
        (
            Key::public(text(&t["rsa_public"])).expect("rsa"),
            &["RS256", "RS512"],
        ),
        (
            Key::public(text(&t["p256_public"])).expect("p256"),
            &["ES256"],
        ),
        (
            Key::public(text(&t["p384_public"])).expect("p384"),
            &["ES384"],
        ),
    ];
    for (key, algs) in cases {
        let r = rig_for(key);
        for alg in algs {
            let got = bearer(&r, text(&t["valid"][*alg]), AUTHINFO);
            assert_eq!(got.0, 200, "{alg}: {got:?}");
            let body: Value = serde_json::from_str(&got.1).expect("json");
            assert_eq!(body["user_name"], "jwt-user", "{alg}");
            assert_eq!(
                body["backend_roles"],
                serde_json::json!(["analyst"]),
                "{alg}"
            );
        }
    }
}

#[test]
fn a_token_that_is_not_valid_is_401() {
    let t = tokens();
    let r = secret_rig();
    make_user(&r, "u", "Zz9!longEnough");
    for (name, token) in t["invalid"].as_object().expect("object") {
        if name == "oversize" {
            continue;
        }
        // Each failure lands in its own throttle window.
        r.now.fetch_add(11_000, std::sync::atomic::Ordering::SeqCst);
        let got = bearer(&r, text(token), AUTHINFO);
        assert_eq!(got, (401, "Unauthorized".to_owned()), "{name}");
    }
}

#[test]
fn the_key_decides_the_algorithm() {
    let t = tokens();
    // An RSA-signed token against an HMAC secret, an HMAC token against an
    // RSA key, and an RSA public key used as the HMAC secret.
    let hmac = secret_rig();
    assert_eq!(bearer(&hmac, text(&t["valid"]["RS256"]), AUTHINFO).0, 401);
    let rsa = rig_for(Key::public(text(&t["rsa_public"])).expect("rsa"));
    assert_eq!(bearer(&rsa, text(&t["valid"]["HS256"]), AUTHINFO).0, 401);
    assert_eq!(
        bearer(
            &rsa,
            text(&t["invalid"]["rsa_public_as_hmac_secret"]),
            AUTHINFO
        )
        .0,
        401
    );
    assert_eq!(
        bearer(&rsa, text(&t["invalid"]["alg_none"]), AUTHINFO).0,
        401
    );
    let ec = rig_for(Key::public(text(&t["p256_public"])).expect("p256"));
    assert_eq!(bearer(&ec, text(&t["valid"]["ES384"]), AUTHINFO).0, 401);
}

#[test]
fn a_token_acts_with_the_roles_its_claim_maps_to() {
    let t = tokens();
    let r = secret_rig();
    let token = text(&t["valid"]["HS256"]);
    let none = bearer(&r, token, AUTHINFO);
    assert_eq!(
        serde_json::from_str::<Value>(&none.1).expect("json")["roles"],
        serde_json::json!([])
    );
    assert_eq!(
        bearer(&r, token, &format!("{API}/roles")).0,
        403,
        "no admin role yet"
    );
    let map = call(
        &r,
        Some(ADMIN),
        "PUT",
        &format!("{API}/rolesmapping/all_access"),
        Some(r#"{"backend_roles":["analyst"]}"#),
    );
    assert!(map.0 == 200 || map.0 == 201, "{map:?}");
    let mapped = bearer(&r, token, AUTHINFO);
    let body: Value = serde_json::from_str(&mapped.1).expect("json");
    assert_eq!(body["roles"], serde_json::json!(["all_access"]));
    assert_eq!(
        bearer(&r, token, &format!("{API}/roles")).0,
        200,
        "the mapping reaches the token"
    );
}

#[test]
fn concurrent_tokens_for_one_subject_do_not_share_roles() {
    let t = tokens();
    let r = secret_rig();
    let map = call(
        &r,
        Some(ADMIN),
        "PUT",
        &format!("{API}/rolesmapping/all_access"),
        Some(r#"{"backend_roles":["analyst"]}"#),
    );
    assert!(map.0 == 200 || map.0 == 201, "{map:?}");
    let (analyst, ops) = (text(&t["valid"]["HS256"]), text(&t["other_roles"]));
    for _ in 0..3 {
        assert_eq!(bearer(&r, analyst, &format!("{API}/roles")).0, 200);
        assert_eq!(bearer(&r, ops, &format!("{API}/roles")).0, 403);
    }
}

#[test]
fn a_token_cannot_claim_a_stored_users_name() {
    let t = tokens();
    let r = secret_rig();
    make_user(&r, "u", "Zz9!longEnough");
    assert_eq!(
        bearer(&r, text(&t["invalid"]["subject_stored_user"]), AUTHINFO).0,
        401
    );
    assert_eq!(
        bearer(&r, text(&t["invalid"]["subject_admin"]), AUTHINFO).0,
        401
    );
}

#[test]
fn a_token_does_not_run_bcrypt_or_read_the_store() {
    let t = tokens();
    let r = secret_rig();
    bearer(&r, text(&t["valid"]["HS256"]), AUTHINFO);
    let total = |r: &Rig| -> u64 { S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum() };
    let before = total(&r);
    for _ in 0..20 {
        assert_eq!(bearer(&r, text(&t["valid"]["HS256"]), AUTHINFO).0, 200);
    }
    assert_eq!(
        total(&r),
        before,
        "a token request reads nothing inside the interval"
    );
}

#[test]
fn authinfo_and_account_answer_a_token_identity() {
    let t = tokens();
    let r = secret_rig();
    let token = text(&t["valid"]["HS256"]);
    let account = bearer(&r, token, &format!("{API}/account"));
    assert_eq!(account.0, 200, "{account:?}");
    let body: Value = serde_json::from_str(&account.1).expect("json");
    assert_eq!(body["user_name"], "jwt-user");
    assert_eq!(body["is_internal_user"], false);
    assert_eq!(body["backend_roles"], serde_json::json!(["analyst"]));
    let put = call(
        &r,
        Some(&format!("raw:Bearer {token}")),
        "PUT",
        &format!("{API}/account"),
        Some(r#"{"current_password":"a","password":"b"}"#),
    );
    assert_eq!(put.0, 403, "{put:?}");
}

#[test]
fn securityconfig_lists_the_jwt_domain_without_its_key() {
    let r = secret_rig();
    let got = call(
        &r,
        Some(ADMIN),
        "GET",
        &format!("{API}/securityconfig"),
        None,
    );
    assert_eq!(got.0, 200, "{}", got.1);
    let body: Value = serde_json::from_str(&got.1).expect("json");
    let domain = &body["config"]["dynamic"]["authc"]["jwt_auth_domain"];
    assert_eq!(domain["http_authenticator"]["type"], "jwt");
    assert_eq!(
        domain["http_authenticator"]["config"]["signing_key"],
        "******"
    );
    assert!(!got.1.contains(text(&tokens()["hs_secret"])));
    assert!(body["config"]["dynamic"]["authc"]["basic_internal_auth_domain"].is_object());
}

#[test]
fn without_a_key_a_token_is_401() {
    let t = tokens();
    let r = crate::security_authn::rig();
    assert_eq!(bearer(&r, text(&t["valid"]["HS256"]), AUTHINFO).0, 401);
}

#[test]
fn failed_tokens_are_throttled_in_one_bucket() {
    let t = tokens();
    let r = secret_rig();
    let bad = text(&t["invalid"]["tampered"]);
    let codes: Vec<u16> = (0..7).map(|_| bearer(&r, bad, AUTHINFO).0).collect();
    assert_eq!(codes[..5], [401; 5]);
    assert_eq!(codes[5], 429, "{codes:?}");
    // A valid token is throttled too while the bucket is full, and a basic
    // user is not affected.
    assert_eq!(bearer(&r, text(&t["valid"]["HS256"]), AUTHINFO).0, 429);
    assert_eq!(call(&r, Some(ADMIN), "GET", AUTHINFO, None).0, 200);
}
