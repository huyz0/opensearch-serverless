//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;

fn ok(_: &mut Request<'_>, _: &mut Response<'_>) -> Result<(), RestError> {
    Ok(())
}

fn segs(path: &str) -> Vec<&str> {
    split(path).map(|r| &path[r]).collect()
}

#[test]
fn paths_split_like_java() {
    assert_eq!(segs("/"), Vec::<&str>::new());
    assert_eq!(segs(""), Vec::<&str>::new());
    assert_eq!(segs("/_search"), ["_search"]);
    assert_eq!(segs("/_search/"), ["_search"]);
    assert_eq!(segs("/a//b"), ["a", "", "b"]);
    assert_eq!(segs("//a"), ["", "a"]);
    assert_eq!(segs("a/b"), ["a", "b"]);
    assert_eq!(segs("/a/b///"), ["a", "b"]);
}

fn router() -> Router {
    Router::new()
        .route(Method::Get, "/_search", &["q"], ok)
        .and_then(|r| r.route(Method::Post, "/_search", &["q"], ok))
        .and_then(|r| r.route(Method::Get, "/{index}/_search", &["q", "size"], ok))
        .and_then(|r| r.route(Method::Get, "/{index}/{type}", &[], ok))
        .and_then(|r| r.route(Method::Get, "/_cat/{what}", &[], ok))
        .and_then(|r| r.route(Method::Put, "/{index}", &[], ok))
        .and_then(|r| r.route(Method::Get, "/", &[], ok))
        .expect("routes register")
}

fn found(r: &Router, m: Method, path: &str) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let mut bound = Vec::new();
    match r.resolve(Some(m), path, &mut bound) {
        Resolved::Found(route) => Some((
            route.pattern(),
            bound
                .iter()
                .map(|(n, rg)| (*n, path[rg.clone()].to_owned()))
                .collect(),
        )),
        _ => None,
    }
}

#[test]
fn explicit_segments_beat_patterns() {
    let r = router();
    assert_eq!(
        found(&r, Method::Get, "/_search").map(|f| f.0),
        Some("/_search")
    );
    assert_eq!(
        found(&r, Method::Get, "/logs/_search"),
        Some(("/{index}/_search", vec![("index", "logs".to_owned())]))
    );
    assert_eq!(
        found(&r, Method::Get, "/_cat/indices").map(|f| f.0),
        Some("/_cat/{what}")
    );
    assert_eq!(
        found(&r, Method::Get, "/logs/x"),
        Some((
            "/{index}/{type}",
            vec![("index", "logs".to_owned()), ("type", "x".to_owned())]
        ))
    );
    assert_eq!(found(&r, Method::Get, "/").map(|f| f.0), Some("/"));
    assert_eq!(
        found(&r, Method::Get, "/logs/_search/").map(|f| f.0),
        Some("/{index}/_search")
    );
}

#[test]
fn other_methods_and_no_match() {
    let r = router();
    let mut bound = Vec::new();
    // `/{index}` matches `/_search` too, so PUT is allowed, as in Java.
    match r.resolve(Some(Method::Delete), "/_search", &mut bound) {
        Resolved::OtherMethods(m) => assert_eq!(m.to_string(), "[GET, POST, PUT]"),
        _ => panic!("expected other methods"),
    }
    match r.resolve(None, "/_search", &mut bound) {
        Resolved::OtherMethods(m) => assert_eq!(m.to_string(), "[GET, POST, PUT]"),
        _ => panic!("expected other methods"),
    }
    assert!(matches!(
        r.resolve(Some(Method::Get), "/a/b/c/d", &mut bound),
        Resolved::NotFound
    ));
    // `/logs` matches the PUT route only.
    match r.resolve(Some(Method::Get), "/logs", &mut bound) {
        Resolved::OtherMethods(m) => assert_eq!(m.to_string(), "[PUT]"),
        _ => panic!("expected other methods"),
    }
}

#[test]
fn accepted_parameters_include_path_parameters() {
    let r = router();
    let mut bound = Vec::new();
    let Resolved::Found(route) = r.resolve(Some(Method::Get), "/i/_search", &mut bound) else {
        panic!("found");
    };
    assert!(route.accepts("size") && route.accepts("index") && !route.accepts("from"));
    assert_eq!(
        route.candidates().collect::<Vec<_>>(),
        ["q", "size", "index"]
    );
}

#[test]
fn bad_and_duplicate_patterns_are_refused() {
    let dup = Router::new()
        .route(Method::Get, "/{a}/x", &[], ok)
        .and_then(|r| r.route(Method::Get, "/{b}/x", &[], ok));
    assert_eq!(
        dup.err(),
        Some(RouteError::Duplicate {
            method: Method::Get,
            pattern: "/{b}/x"
        })
    );
    assert!(
        Router::new()
            .route(Method::Get, "/{a}/x", &[], ok)
            .and_then(|r| r.route(Method::Post, "/{b}/x", &[], ok))
            .is_ok()
    );
    for bad in ["/{}", "/a{b}", "/{a", "/{{a}}"] {
        assert_eq!(
            Router::new().route(Method::Get, bad, &[], ok).err(),
            Some(RouteError::BadPattern(bad))
        );
    }
}

#[test]
fn the_segment_limit_holds_at_max_and_refuses_one_more() {
    let at = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p";
    let over = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q";
    assert!(Router::new().route(Method::Get, at, &[], ok).is_ok());
    assert_eq!(
        Router::new().route(Method::Get, over, &[], ok).err(),
        Some(RouteError::TooManySegments(over))
    );
}

#[test]
fn the_first_position_that_differs_decides_which_route_is_more_explicit() {
    let r = Router::new()
        .route(Method::Get, "/{a}/lit", &[], ok)
        .and_then(|r| r.route(Method::Get, "/lit/{b}", &[], ok))
        .expect("routes register");
    let (param_first, literal_first) = (&r.routes[0], &r.routes[1]);
    assert!(more_explicit(literal_first, param_first));
    assert!(!more_explicit(param_first, literal_first));
    assert!(!more_explicit(param_first, param_first));
}
