//! Which actions a user's roles grant (docs/specs/security-authz/SPEC.md).

use oss_coordinator::security::{Config, Kind};
use oss_doc::mapping::json::parse;

fn doc(text: &str) -> oss_doc::mapping::json::Json {
    parse(text.as_bytes()).expect("json")
}

/// A config with user `u` holding role `r` made from `role` (and groups).
fn config(role: &str, groups: &[(&str, &str)]) -> Config {
    let mut c = Config::new();
    c.add(Kind::InternalUsers, "u", &doc(r#"{"hash":"x"}"#));
    c.add(Kind::Roles, "r", &doc(role));
    c.add(Kind::RolesMapping, "r", &doc(r#"{"users":["u"]}"#));
    for (name, body) in groups {
        c.add(Kind::ActionGroups, name, &doc(body));
    }
    c
}

fn logs_reader() -> Config {
    config(
        r#"{"cluster_permissions":["cluster_monitor","indices:data/write/bulk"],
            "index_permissions":[{"index_patterns":["logs-*"],"allowed_actions":["read","write"]}]}"#,
        &[],
    )
}

const SEARCH: &str = "indices:data/read/search";

// docs/specs/security-authz AC-1
#[test]
fn a_granted_pattern_allows_its_names_and_only_them() {
    let c = logs_reader();
    assert!(c.indices_allow("u", SEARCH, &["logs-1"]));
    assert!(!c.indices_allow("u", SEARCH, &["other"]));
    assert!(!c.indices_allow("u", "indices:admin/delete", &["logs-1"]));
    assert!(!c.indices_allow("nobody", SEARCH, &["logs-1"]));
}

// docs/specs/security-authz AC-2
#[test]
fn a_wildcard_request_needs_a_pattern_that_covers_it() {
    let c = logs_reader();
    assert!(c.indices_allow("u", SEARCH, &["logs-*"]));
    assert!(c.indices_allow("u", SEARCH, &["logs-2024-*"]));
    assert!(!c.indices_allow("u", SEARCH, &["l*"]));
    assert!(!c.indices_allow("u", SEARCH, &["*"]));
    assert!(
        !c.indices_allow("u", SEARCH, &[]),
        "no index means all of them"
    );
    assert!(!c.indices_allow("u", SEARCH, &["compat-sec-*"]));
    let narrow = config(
        r#"{"index_permissions":[{"index_patterns":["a?"],"allowed_actions":["read"]}]}"#,
        &[],
    );
    assert!(narrow.indices_allow("u", SEARCH, &["ab"]));
    assert!(
        !narrow.indices_allow("u", SEARCH, &["a*"]),
        "`a?` does not cover `a*`, which matches `abc`"
    );
    assert!(!narrow.indices_allow("u", SEARCH, &["a?c"]));
}

// docs/specs/security-authz AC-2
#[test]
fn every_index_of_a_list_must_be_covered() {
    let c = logs_reader();
    assert!(c.indices_allow("u", SEARCH, &["logs-1", "logs-2"]));
    assert!(!c.indices_allow("u", SEARCH, &["logs-1", "payroll"]));
}

// docs/specs/security-authz AC-2: the claim "a pattern covers a request" against
// a brute-force reading of both over a small alphabet.
#[test]
fn covering_agrees_with_every_string_the_request_stands_for() {
    fn matches(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| matches(rest, &t[i..])),
            Some(('?', rest)) => !t.is_empty() && matches(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && matches(rest, &t[1..]),
        }
    }
    let alphabet = ['a', 'b', '*', '?'];
    let mut exprs: Vec<String> = vec![String::new()];
    for _ in 0..4 {
        let next: Vec<String> = exprs
            .iter()
            .flat_map(|e| alphabet.iter().map(move |c| format!("{e}{c}")))
            .collect();
        exprs.extend(next);
        exprs.sort();
        exprs.dedup();
    }
    // Every string over {a,b,c} up to length 6 stands in for the request's matches.
    let mut universe: Vec<Vec<char>> = vec![vec![]];
    for _ in 0..6 {
        let next: Vec<Vec<char>> = universe
            .iter()
            .flat_map(|s| {
                ['a', 'b', 'c'].iter().map(move |c| {
                    let mut s = s.clone();
                    s.push(*c);
                    s
                })
            })
            .collect();
        universe.extend(next);
        universe.sort();
        universe.dedup();
    }
    let mut checked = 0;
    for pattern in exprs.iter().filter(|p| !p.is_empty()) {
        let body = format!(
            r#"{{"index_permissions":[{{"index_patterns":["{pattern}"],"allowed_actions":["read"]}}]}}"#
        );
        let c = config(&body, &[]);
        for request in exprs.iter().filter(|r| !r.is_empty()) {
            if !c.indices_allow("u", SEARCH, &[request]) {
                continue;
            }
            checked += 1;
            let p: Vec<char> = pattern.chars().collect();
            let r: Vec<char> = request.chars().collect();
            for s in universe.iter().filter(|s| matches(&r, s)) {
                assert!(
                    matches(&p, s),
                    "`{pattern}` allowed `{request}` yet does not match {s:?}"
                );
            }
        }
    }
    assert!(checked > 100, "the property was exercised: {checked}");
}

// docs/specs/security-authz AC-3
#[test]
fn cluster_permissions_and_action_groups_decide_cluster_actions() {
    let c = logs_reader();
    assert!(c.cluster_allows("u", "cluster:monitor/health"));
    assert!(c.cluster_allows("u", "indices:data/write/bulk"));
    assert!(!c.cluster_allows("u", "cluster:admin/settings/update"));
}

// docs/specs/security-authz AC-5
#[test]
fn groups_expand_to_any_depth_and_a_cycle_ends() {
    let c = config(
        r#"{"index_permissions":[{"index_patterns":["x"],"allowed_actions":["outer"]}]}"#,
        &[
            ("outer", r#"{"allowed_actions":["inner"]}"#),
            (
                "inner",
                r#"{"allowed_actions":["indices:data/read/get","outer"]}"#,
            ),
        ],
    );
    assert!(c.indices_allow("u", "indices:data/read/get", &["x"]));
    assert!(
        !c.indices_allow("u", "indices:data/read/search", &["x"]),
        "the cycle ends"
    );
    let bare = config(
        r#"{"index_permissions":[{"index_patterns":["x"],"allowed_actions":["indices:data/read/search"]}]}"#,
        &[],
    );
    assert!(
        bare.indices_allow("u", SEARCH, &["x"]),
        "a name that is no group is an action"
    );
}

// docs/specs/security-authz AC-5
#[test]
fn a_stored_group_or_role_does_not_replace_a_built_in() {
    let mut c = config(
        r#"{"index_permissions":[{"index_patterns":["x"],"allowed_actions":["read"]}]}"#,
        &[("read", r#"{"allowed_actions":["*"]}"#)],
    );
    assert!(
        !c.indices_allow("u", "indices:data/write/index", &["x"]),
        "`read` is still the built-in"
    );
    c.add(
        Kind::Roles,
        "all_access",
        &doc(r#"{"cluster_permissions":[],"index_permissions":[]}"#),
    );
    c.add(Kind::RolesMapping, "all_access", &doc(r#"{"users":["u"]}"#));
    assert!(
        c.cluster_allows("u", "cluster:admin/anything"),
        "the built-in all_access stands"
    );
}

// docs/specs/security-authz AC-7
#[test]
fn the_administrator_may_do_everything_and_user_names_in_patterns_are_literal() {
    let c = Config::new().with_admin("root", "h");
    assert!(c.cluster_allows("root", "cluster:admin/settings/update"));
    assert!(c.indices_allow("root", "indices:admin/delete", &["any", "*"]));
    assert!(c.indices_allow("root", SEARCH, &[]));
    let mut own = Config::new();
    own.add(Kind::InternalUsers, "al", &doc(r#"{"hash":"x"}"#));
    own.add(Kind::InternalUsers, "*", &doc(r#"{"hash":"x"}"#));
    own.add(
        Kind::Roles,
        "own",
        &doc(r#"{"index_permissions":[{"index_patterns":["home-${user.name}"],"allowed_actions":["read"]}]}"#),
    );
    own.add(Kind::RolesMapping, "own", &doc(r#"{"users":["*"]}"#));
    assert!(own.indices_allow("al", SEARCH, &["home-al"]));
    assert!(!own.indices_allow("al", SEARCH, &["home-bo"]));
    assert!(
        !own.indices_allow("*", SEARCH, &["home-bo"]),
        "a user name holding a wildcard voids the pattern"
    );
}

// docs/specs/security-authz AC-4
#[test]
fn the_refusal_is_the_recorded_security_exception() {
    use oss_coordinator::security::forbidden;
    let mut c = Config::new();
    c.add(
        Kind::InternalUsers,
        "u",
        &doc(r#"{"hash":"x","backend_roles":["b1","b2"]}"#),
    );
    let body: serde_json::Value =
        serde_json::from_str(&forbidden(&c, "u", "indices:data/read/search")).expect("json");
    let reason = "no permissions for [indices:data/read/search] and User [name=u, backend_roles=[b1, b2], requestedTenant=null]";
    assert_eq!(
        body,
        serde_json::json!({
            "error": {
                "root_cause": [{"type": "security_exception", "reason": reason}],
                "type": "security_exception",
                "reason": reason
            },
            "status": 403
        })
    );
}

// docs/specs/security-authz AC-7
#[test]
fn all_access_is_the_star_on_both_levels_and_nothing_less() {
    let c = Config::new().with_admin("root", "h");
    assert!(c.all_access("root"));
    assert!(!logs_reader().all_access("u"));
    let wide = config(
        r#"{"cluster_permissions":["cluster:*"],"index_permissions":[{"index_patterns":["*"],"allowed_actions":["indices:*"]}]}"#,
        &[],
    );
    assert!(
        !wide.all_access("u"),
        "`cluster:*` and `indices:*` are not `*`"
    );
    let star = config(
        r#"{"cluster_permissions":["*"],"index_permissions":[{"index_patterns":["*"],"allowed_actions":["*"]}]}"#,
        &[],
    );
    assert!(star.all_access("u"));
}

// docs/specs/security-authz AC-2: a permission this node cannot enforce grants nothing.
#[test]
fn a_permission_with_dls_fls_or_masking_grants_nothing() {
    for extra in [
        r#""dls":"{\"term\":{\"o\":\"x\"}}""#,
        r#""fls":["~secret"]"#,
        r#""masked_fields":["ssn"]"#,
    ] {
        let role = format!(
            r#"{{"index_permissions":[{{"index_patterns":["docs*"],"allowed_actions":["read"],{extra}}}]}}"#
        );
        let c = config(&role, &[]);
        assert!(!c.indices_allow("u", SEARCH, &["docs1"]), "{extra}");
    }
    let plain = config(
        r#"{"index_permissions":[{"index_patterns":["docs*"],"allowed_actions":["read"],"dls":"","fls":[],"masked_fields":[]}]}"#,
        &[],
    );
    assert!(
        plain.indices_allow("u", SEARCH, &["docs1"]),
        "empty is none"
    );
}

// docs/specs/security-authz AC-1: `*` among a mapping's backend roles needs the user to have one.
#[test]
fn a_star_backend_role_does_not_reach_a_user_with_none() {
    let mut c = config(r#"{}"#, &[]);
    c.add(
        Kind::RolesMapping,
        "star",
        &doc(r#"{"backend_roles":["*"]}"#),
    );
    assert!(!c.roles_of("u").contains(&"star".to_owned()));
    c.add(
        Kind::InternalUsers,
        "v",
        &doc(r#"{"hash":"x","backend_roles":["b"]}"#),
    );
    assert!(c.roles_of("v").contains(&"star".to_owned()));
}
