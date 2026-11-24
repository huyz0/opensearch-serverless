//! `aws-chunked` bodies on the front end: the object is the decoded bytes
//! (docs/specs/s3-aws-chunked/SPEC.md).

use crate::server::{exchange, signed, status};
use crate::{BUCKET, fixture, get_all};
use oss_blob::*;

/// A PUT whose body is `aws-chunked`, as the AWS SDKs send a payload they
/// sign or checksum while it streams: the object is the decoded bytes.
fn chunked_put(endpoint: &str, key: &str, wire: &str, decoded: usize) -> String {
    let (length, decoded) = (wire.len().to_string(), decoded.to_string());
    let head = signed(
        endpoint,
        "PUT",
        &format!("/{BUCKET}/{key}"),
        &[
            ("content-encoding", "aws-chunked"),
            ("x-amz-decoded-content-length", decoded.as_str()),
            ("content-length", length.as_str()),
        ],
    );
    exchange(endpoint, format!("{head}{wire}").as_bytes())
}

#[test]
fn an_aws_chunked_put_stores_the_decoded_bytes() {
    let f = fixture();
    let ep = f.server.endpoint();
    let signed_chunks = "5;chunk-signature=aa\r\nhello\r\n6;chunk-signature=bb\r\n world\r\n0;chunk-signature=cc\r\n\r\n";
    let r = chunked_put(&ep, "signed", signed_chunks, 11);
    assert_eq!(status(&r), "200", "{r}");
    let trailers = "5\r\nhello\r\n6\r\n world\r\n0\r\nx-amz-checksum-crc32c:AAAAAA==\r\n\r\n";
    let r = chunked_put(&ep, "trailer", trailers, 11);
    assert_eq!(status(&r), "200", "{r}");
    let r = chunked_put(&ep, "empty", "0\r\n\r\n", 0);
    assert_eq!(status(&r), "200", "{r}");
    for key in ["signed", "trailer"] {
        assert_eq!(
            get_all(f.server.store(), key, ByteRange::Full).unwrap(),
            b"hello world",
            "{key}"
        );
    }
    assert!(
        get_all(f.server.store(), "empty", ByteRange::Full)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_malformed_aws_chunked_body_stores_nothing() {
    let f = fixture();
    let ep = f.server.endpoint();
    for (key, wire) in [
        ("short", "5\r\nhel"),
        ("badsize", "zz\r\nhello\r\n0\r\n\r\n"),
        ("nocrlf", "5\r\nhelloXX0\r\n\r\n"),
    ] {
        let r = chunked_put(&ep, key, wire, 5);
        assert_ne!(status(&r), "200", "{key}: {r}");
        assert!(
            get_all(f.server.store(), key, ByteRange::Full).is_err(),
            "{key}"
        );
    }
}
