//! The `rest` codec (docs/specs/plugin-system AC-5).

use oss_ext::rest::{CodecError, Method, RestRequest, RestResponse};

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_owned(), b.to_owned())
}

fn request() -> RestRequest {
    RestRequest {
        method: Method::Post,
        tenant: "acme".into(),
        action: "read".into(),
        path_params: vec![pair("place", "oslo")],
        query: vec![pair("size", "3")],
        headers: vec![pair("content-type", "application/json")],
        body: b"{}".to_vec(),
    }
}

// A golden vector: these bytes are the contract, so a change to them is a
// break of plugin API 1.x.
const GOLDEN_REQUEST: &[u8] = &[
    1, 2, 4, 0, 0, 0, b'a', b'c', b'm', b'e', 4, 0, 0, 0, b'r', b'e', b'a', b'd', 1, 0, 5, 0, 0, 0,
    b'p', b'l', b'a', b'c', b'e', 4, 0, 0, 0, b'o', b's', b'l', b'o', 1, 0, 4, 0, 0, 0, b's', b'i',
    b'z', b'e', 1, 0, 0, 0, b'3', 1, 0, 12, 0, 0, 0, b'c', b'o', b'n', b't', b'e', b'n', b't',
    b'-', b't', b'y', b'p', b'e', 16, 0, 0, 0, b'a', b'p', b'p', b'l', b'i', b'c', b'a', b't',
    b'i', b'o', b'n', b'/', b'j', b's', b'o', b'n', 2, 0, 0, 0, b'{', b'}',
];

#[test]
fn a_request_is_the_golden_bytes_and_reads_back() {
    assert_eq!(request().encode(), GOLDEN_REQUEST);
    assert_eq!(RestRequest::decode(GOLDEN_REQUEST, 2), Ok(request()));
}

#[test]
fn a_response_round_trips_and_its_golden_bytes_hold() {
    let r = RestResponse {
        status: 201,
        headers: vec![pair("x", "y")],
        body: b"ok".to_vec(),
    };
    let bytes = r.encode();
    assert_eq!(
        bytes,
        [
            1, 201, 0, 1, 0, 1, 0, 0, 0, b'x', 1, 0, 0, 0, b'y', 2, 0, 0, 0, b'o', b'k'
        ]
    );
    assert_eq!(RestResponse::decode(&bytes, 2), Ok(r));
}

#[test]
fn every_malformed_input_is_refused_with_its_reason() {
    let ok = request().encode();
    assert_eq!(RestRequest::decode(&[], 9), Err(CodecError::Short));
    let mut v = ok.clone();
    v[0] = 2;
    assert_eq!(RestRequest::decode(&v, 9), Err(CodecError::Version));
    let mut m = ok.clone();
    m[1] = 9;
    assert_eq!(RestRequest::decode(&m, 9), Err(CodecError::Value));
    let mut t = ok.clone();
    t.push(0);
    assert_eq!(RestRequest::decode(&t, 9), Err(CodecError::Trailing));
    assert_eq!(
        RestRequest::decode(&ok[..ok.len() - 1], 9),
        Err(CodecError::Short)
    );
    // The body is one byte over the ceiling.
    assert_eq!(RestRequest::decode(&ok, 1), Err(CodecError::Bound));
    assert!(RestRequest::decode(&ok, 2).is_ok());
    // Not UTF-8 in the tenant.
    let mut bad = ok.clone();
    bad[6] = 0xff;
    assert_eq!(RestRequest::decode(&bad, 9), Err(CodecError::Text));
    // A string longer than the part ceiling is refused before it is read.
    let mut big = vec![1, 1];
    big.extend_from_slice(&(oss_ext::rest::MAX_PART_BYTES as u32 + 1).to_le_bytes());
    assert_eq!(RestRequest::decode(&big, 9), Err(CodecError::Bound));
    // More pairs than the ceiling.
    let mut many = vec![1, 1, 0, 0, 0, 0, 0, 0, 0, 0];
    many.extend_from_slice(&(oss_ext::rest::MAX_PAIRS as u16 + 1).to_le_bytes());
    assert_eq!(RestRequest::decode(&many, 9), Err(CodecError::Bound));
    // Exactly the ceiling of pairs is read.
    let full = RestRequest {
        query: (0..oss_ext::rest::MAX_PAIRS)
            .map(|i| pair(&i.to_string(), "v"))
            .collect(),
        ..request()
    };
    assert_eq!(RestRequest::decode(&full.encode(), 2), Ok(full));
}

#[test]
fn a_response_status_must_be_an_http_status() {
    for s in [0_u16, 99, 600, 1000] {
        let r = RestResponse {
            status: s,
            headers: vec![],
            body: vec![],
        };
        assert_eq!(
            RestResponse::decode(&r.encode(), 0),
            Err(CodecError::Value),
            "{s}"
        );
    }
    for s in [100_u16, 599] {
        let r = RestResponse {
            status: s,
            headers: vec![],
            body: vec![],
        };
        assert!(RestResponse::decode(&r.encode(), 0).is_ok(), "{s}");
    }
}

#[test]
fn methods_parse_by_name() {
    for (n, m) in [
        ("GET", Method::Get),
        ("POST", Method::Post),
        ("PUT", Method::Put),
        ("DELETE", Method::Delete),
        ("HEAD", Method::Head),
    ] {
        assert_eq!(Method::parse(n), Some(m));
    }
    assert_eq!(Method::parse("get"), None);
    assert_eq!(Method::parse("PATCH"), None);
}

#[test]
fn every_method_code_reads_back_as_its_method() {
    for m in [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Delete,
        Method::Head,
    ] {
        let r = RestRequest {
            method: m,
            ..request()
        };
        assert_eq!(RestRequest::decode(&r.encode(), 2).map(|r| r.method), Ok(m));
    }
}
