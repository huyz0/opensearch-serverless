use super::*;

// docs/specs/security-authz AC-6
#[test]
fn a_route_with_no_row_and_a_prefix_route_need_all_access() {
    assert_eq!(rule(Some(Method::Get), "/new/{index}/route"), None);
    assert_eq!(
        rule_of(false, Some(Method::Get), "/new/{index}/route"),
        Rule::AllAccess
    );
    assert_eq!(rule_of(true, None, "/_plugins/_ml"), Rule::AllAccess);
    assert_eq!(
        rule_of(false, Some(Method::Get), "/{index}/_search"),
        Rule::Index("indices:data/read/search")
    );
    assert_eq!(
        rule_of(false, Some(Method::Delete), "/{index}/_search"),
        Rule::AllAccess,
        "a method with no row for a known pattern is not allowed through"
    );
}

// docs/specs/stock-snapshot-restore AC-7: registering a stock repository
// is a cluster-admin act, as every repository and snapshot route is.
#[test]
fn every_repository_and_snapshot_route_needs_all_access() {
    for method in [Method::Put, Method::Post, Method::Get, Method::Delete] {
        for pattern in [
            "/_snapshot/{repo}",
            "/_snapshot/{repo}/_verify",
            "/_snapshot/{repo}/{name}",
            "/_snapshot/{repo}/{name}/_restore",
        ] {
            assert_eq!(rule_of(false, Some(method), pattern), Rule::AllAccess);
        }
    }
}
