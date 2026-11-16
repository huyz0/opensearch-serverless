//! Batch deletes over HTTP: `POST /bucket?delete`, one request a thousand
//! keys, with the body's `Content-MD5` (docs/specs/blob-batch-delete/SPEC.md).

use std::io::Write;

use crate::server::{connect, exchange, read_to_close, signed, status};
use crate::{BUCKET, assert_counts, fixture, put};
use oss_blob::*;

/// Keys an XML body must escape or carry as UTF-8.
const ODD: [&str; 4] = ["a&b<c>\"d'e", "ü/ ✓ space", "x]]>y", "tab\there"];

// AC-1, AC-4
#[test]
fn a_batch_delete_is_one_post_a_thousand_keys_on_client_and_server() {
    let f = fixture();
    let mut all: Vec<String> = (0..2_500).map(|i| format!("d/{i:05}")).collect();
    all.extend(ODD.iter().map(|k| (*k).to_owned()));
    for k in &all {
        put(f.server.store(), k, b"x");
    }
    put(f.server.store(), "kept", b"x");
    let before = f.server.requests();
    let mut named: Vec<&str> = all.iter().map(String::as_str).collect();
    named.push("never/there");
    f.client.delete_many(&named).unwrap();
    assert_eq!(f.server.store().object_count(), 1, "only the key not named");
    assert_counts(f.client.meter(), &[(S3Op::Delete, 3)]);
    assert_eq!(f.server.requests() - before, 3);
    assert_eq!(f.server.store().meter().count(S3Op::Delete), 3);
    assert_eq!(f.client.meter().errors(), 0);
}

// AC-2
#[test]
fn a_batch_delete_checks_every_key_before_any_request() {
    let f = fixture();
    let long = "k".repeat(MAX_KEY_BYTES + 1);
    for bad in ["", long.as_str()] {
        assert!(matches!(
            f.client.delete_many(&["a", bad]),
            Err(BlobError::InvalidArgument(_))
        ));
    }
    f.client.delete_many(&[]).unwrap();
    assert_eq!(f.server.requests(), 0);
    assert_eq!(f.client.meter().total(), 0);
}

// AC-3
#[test]
fn a_failed_batch_is_retried_and_each_attempt_counted() {
    let f = fixture();
    put(f.server.store(), "a", b"x");
    f.server.answer_next(503, "SlowDown", 2);
    f.client.delete_many(&["a"]).unwrap();
    assert_eq!(f.server.store().object_count(), 0);
    assert_counts(f.client.meter(), &[(S3Op::Delete, 3)]);
    assert_eq!(f.client.meter().errors(), 2);
    let f = fixture();
    put(f.server.store(), "a", b"x");
    f.server.answer_next(403, "AccessDenied", 1);
    assert_eq!(f.client.delete_many(&["a"]), Err(BlobError::Denied));
    assert_eq!(f.server.store().object_count(), 1);
    assert_counts(f.client.meter(), &[(S3Op::Delete, 1)]);
}

/// A batch delete sent by hand: `body`, with `md5` as its digest if given.
fn raw_batch(endpoint: &str, md5: Option<&str>, body: &str) -> String {
    let length = body.len().to_string();
    let mut extra = vec![("content-length", length.as_str())];
    if let Some(md5) = md5 {
        extra.push(("content-md5", md5));
    }
    let head = signed(endpoint, "POST", &format!("/{BUCKET}?delete="), &extra);
    exchange(endpoint, format!("{head}{body}").as_bytes())
}

// AC-4
#[test]
fn the_front_end_refuses_a_batch_as_s3_does() {
    let f = fixture();
    let ep = f.server.endpoint();
    put(f.server.store(), "a", b"x");
    let one = "<Delete><Object><Key>a</Key></Object></Delete>";
    let thousand_and_one: String = std::iter::once("<Delete>".to_owned())
        .chain((0..1_001).map(|i| format!("<Object><Key>k{i}</Key></Object>")))
        .chain(std::iter::once("</Delete>".to_owned()))
        .collect();
    for (md5, body, want, code) in [
        (None, one, "400", "InvalidRequest"),
        (Some("R2cEG0GhQVRNiyXR3QnSyw=="), one, "400", "BadDigest"),
        (
            Some("R2cEG0GhQVRNiyXR3QnSyw=="),
            "<Delete></Delete>",
            "400",
            "MalformedXML",
        ),
        (
            Some("B6h4uO7vUT11E1H/ljtkmA=="),
            "<Delete><Object><Key>a&bad;</Key></Object></Delete>",
            "400",
            "MalformedXML",
        ),
        (
            Some("qH4RsE4XBX79uAjxcaE8IQ=="),
            thousand_and_one.as_str(),
            "400",
            "MalformedXML",
        ),
    ] {
        let r = raw_batch(&ep, md5, body);
        assert_eq!(status(&r), want, "{code}: {r}");
        assert!(r.contains(code), "{code}: {r}");
    }
    assert_eq!(f.server.store().object_count(), 1, "nothing was deleted");
    let r = raw_batch(&ep, Some("aFPqFnlCon58cZ4Qt7zwvQ=="), one);
    assert_eq!(status(&r), "200", "{r}");
    assert!(r.contains("<DeleteResult>"), "{r}");
    assert_eq!(f.server.store().object_count(), 0);
    // Too large to take: refused before its body is read.
    let head = signed(
        &ep,
        "POST",
        &format!("/{BUCKET}?delete="),
        &[("content-length", "9000000"), ("content-md5", "x")],
    );
    let mut s = connect(&ep);
    s.write_all(head.as_bytes()).unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let r = read_to_close(&mut s);
    assert_eq!(status(&r), "400", "{r}");
    assert!(r.contains("MalformedXML"), "{r}");
    // A body shorter than it declared.
    let head = signed(
        &ep,
        "POST",
        &format!("/{BUCKET}?delete="),
        &[("content-length", "50"), ("content-md5", "x")],
    );
    let mut s = connect(&ep);
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(b"<Delete>").unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let r = read_to_close(&mut s);
    assert_eq!(status(&r), "400", "{r}");
    assert!(r.contains("IncompleteBody"), "{r}");
}
