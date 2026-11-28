use super::*;
use oss_mem::Pool;

fn body(e: &RestError) -> serde_json::Value {
    let mut out = Vec::new(); // ALLOC: one small body in a test.
    e.write_body(&mut out).expect("vec");
    serde_json::from_slice(&out).expect("valid json")
}

#[test]
fn an_exception_renders_root_cause_type_reason_and_metadata() {
    let e = RestError::exception(
        400,
        Exception::new(
            "resource_already_exists_exception",
            "index [a/u] already exists",
        )
        .with_meta("index", Meta::Str("a".into()))
        .with_meta("index_uuid", Meta::Str("u".into())),
    );
    let want = serde_json::json!({
        "error": {
            "root_cause": [{"type": "resource_already_exists_exception", "reason": "index [a/u] already exists", "index": "a", "index_uuid": "u"}],
            "type": "resource_already_exists_exception", "reason": "index [a/u] already exists", "index": "a", "index_uuid": "u"
        },
        "status": 400
    });
    assert_eq!(body(&e), want);
    assert_eq!(e.status(), 400);
    assert_eq!(
        e.as_exception().map(Exception::kind),
        Some("resource_already_exists_exception")
    );
    assert!(!e.closes_connection() && !e.is_empty());
}

#[test]
fn the_root_cause_is_the_innermost_and_written_without_its_cause() {
    let e = RestError::exception(
        400,
        Exception::new("outer", "o")
            .caused_by(Exception::new("inner", "i").with_meta("n", Meta::Int(3))),
    );
    let want = serde_json::json!({
        "error": {
            "root_cause": [{"type": "inner", "reason": "i", "n": 3}],
            "type": "outer", "reason": "o",
            "caused_by": {"type": "inner", "reason": "i", "n": 3}
        },
        "status": 400
    });
    assert_eq!(body(&e), want);
    assert_eq!(e.as_exception().map(Exception::reason), Some("o"));
}

#[test]
fn the_root_cause_stops_before_a_jdk_or_library_exception() {
    let jdk = Exception::new("illegal_argument_exception", "bad");
    let e = Exception::new("mid", "m").caused_by(jdk);
    let e = Exception::new("outer", "o").caused_by(e);
    assert_eq!(e.root().kind(), "mid");
    let plain = Exception::new("parse_exception", "p").plain();
    let e = Exception::new("parse_exception", "wrapper").caused_by(plain);
    assert_eq!(e.root().reason(), "wrapper");
    let open = Exception::new("parse_exception", "p");
    let e = Exception::new("parse_exception", "wrapper").caused_by(open);
    assert_eq!(e.root().reason(), "p");
}

#[test]
fn circuit_breaking_matches_the_in_flight_breaker_message() {
    let ex = Exhausted {
        pool: Pool::RequestBodies,
        requested: 65_536,
        in_use: 131_072,
        limit: 131_072,
    };
    let e = RestError::circuit_breaking(&ex, "<http_request>");
    assert_eq!(e.status(), 429);
    let want = serde_json::json!({
        "error": {
            "root_cause": [{
                "type": "circuit_breaking_exception",
                "reason": "[in_flight_requests] Data too large, data for [<http_request>] would be [196608/192kb], which is larger than the limit of [131072/128kb]",
                "bytes_wanted": 196_608, "bytes_limit": 131_072, "durability": "TRANSIENT"
            }],
            "type": "circuit_breaking_exception",
            "reason": "[in_flight_requests] Data too large, data for [<http_request>] would be [196608/192kb], which is larger than the limit of [131072/128kb]",
            "bytes_wanted": 196_608, "bytes_limit": 131_072, "durability": "TRANSIENT"
        },
        "status": 429
    });
    assert_eq!(body(&e), want);
}

#[test]
fn a_named_breaker_names_itself_and_its_durability() {
    let mut ex = Exhausted {
        pool: Pool::OpenIndexes,
        requested: 2048,
        in_use: 0,
        limit: 1024,
    };
    let e = RestError::breaker(&ex, "open_indexes", "<open_index>", true);
    let reason = "[open_indexes] Data too large, data for [<open_index>] would be [2048/2kb]";
    assert!(
        body(&e)["error"]["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with(reason))
    );
    assert_eq!(body(&e)["error"]["durability"], "PERMANENT");
    ex.requested = 1;
    let e = RestError::breaker(&ex, "open_indexes", "<open_index>", false);
    assert_eq!(body(&e)["error"]["durability"], "TRANSIENT");
}

#[test]
fn simple_errors_with_and_without_status() {
    let e = RestError::no_handler("/nope", Method::Get);
    assert_eq!(
        body(&e),
        serde_json::json!({"error": "no handler found for uri [/nope] and method [GET]"})
    );
    let mut allowed = MethodSet::EMPTY;
    allowed.insert(Method::Get);
    allowed.insert(Method::Post);
    let e = RestError::method_not_allowed("/_search", Some(Method::Delete), allowed);
    assert_eq!(
        body(&e),
        serde_json::json!({"error": "Incorrect HTTP method for uri [/_search] and method [DELETE], allowed: [GET, POST]", "status": 405})
    );
    assert_eq!(e.allow(), allowed);
    let e = RestError::method_not_allowed("/x", None, MethodSet::EMPTY);
    assert_eq!(
        body(&e),
        serde_json::json!({"error": "Unexpected HTTP method", "status": 405})
    );
    let e = RestError::simple(406, "Content-Type header [text/plain] is not supported");
    assert_eq!(e.status(), 406);
    assert!(e.as_exception().is_none());
}

#[test]
fn too_large_is_empty_and_closes() {
    let e = RestError::too_large();
    assert_eq!(e.status(), 413);
    assert!(e.is_empty() && e.closes_connection());
    let mut out = Vec::new(); // ALLOC: one small body in a test.
    e.write_body(&mut out).expect("vec");
    assert!(out.is_empty());
}

#[test]
fn io_errors_close_the_connection() {
    let e = RestError::from(io::Error::other("reset"));
    assert_eq!(e.status(), 500);
    assert!(e.closes_connection());
    assert!(
        RestError::illegal_argument("x")
            .closing()
            .closes_connection()
    );
    assert_eq!(
        RestError::json_parse("bad")
            .as_exception()
            .map(Exception::kind),
        Some("json_parse_exception")
    );
}
