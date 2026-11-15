//! The client against responses no correct S3 sends: each is a
//! `Protocol` or `Unavailable` error, never a wrong answer.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

use crate::{config, credentials, get_all};
use oss_blob::*;

/// A server that answers each connection with the next canned response
/// and closes it.
fn canned(responses: Vec<&'static [u8]>) -> (S3Store, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        for response in responses {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && s.read(&mut byte).unwrap() == 1 {
                head.push(byte[0]);
            }
            s.write_all(response).unwrap();
        }
    });
    let mut c = config(&endpoint, credentials());
    c.retry.max_attempts = 1;
    (S3Store::new(c).unwrap(), thread)
}

#[test]
fn responses_that_do_not_fit_the_request_are_protocol_errors() {
    let (s3, t) = canned(vec![
        b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nabc",
        b"HTTP/1.1 206 Partial Content\r\ncontent-length: 3\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        b"HTTP/1.1 412 Precondition Failed\r\ncontent-length: 0\r\n\r\n",
    ]);
    let range = ByteRange::From { start: 0, len: 3 };
    assert_eq!(
        get_all(&s3, "k", range),
        Err(BlobError::Protocol(
            "response does not match the range asked"
        ))
    );
    assert_eq!(
        get_all(&s3, "k", range),
        Err(BlobError::Protocol("no Content-Range total"))
    );
    assert_eq!(
        get_all(&s3, "k", ByteRange::Full),
        Err(BlobError::Protocol("no Content-Length"))
    );
    assert_eq!(s3.head("k"), Err(BlobError::Unexpected(412)));
    t.join().unwrap();
    assert_eq!(s3.meter().errors(), 4);
}

#[test]
fn a_body_shorter_than_declared_is_unavailable() {
    let (s3, t) = canned(vec![
        b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\n",
    ]);
    let mut seen = Vec::new();
    let r = s3.get("k", ByteRange::Full, &mut [0u8; 64], &mut |c: &[u8]| {
        seen.extend_from_slice(c);
        Ok(())
    });
    assert_eq!(r, Err(BlobError::Unavailable));
    assert!(seen.len() <= 3);
    assert_eq!(
        get_all(&s3, "k", ByteRange::Full),
        Err(BlobError::Unavailable)
    );
    t.join().unwrap();
}

#[test]
fn a_malformed_or_cut_list_page_is_refused() {
    let (s3, t) = canned(vec![
        b"HTTP/1.1 200 OK\r\ncontent-length: 21\r\n\r\n<Error>nope</Error>\r\n",
        b"HTTP/1.1 200 OK\r\ncontent-length: 500\r\n\r\n<ListBucketResult><Contents><Key>a</Key><Size>1</Size></Contents>",
        b"HTTP/1.1 200 OK\r\ncontent-length: 500\r\n\r\n<ListBucketResult><Cont",
        b"HTTP/1.1 200 OK\r\ncontent-length: 70\r\n\r\n<ListBucketResult><Contents><Key>a</Key></Contents></ListBucketResult>",
    ]);
    let mut keys = Vec::new();
    let mut visit = |k: &str, _| keys.push(k.to_owned());
    // LIST: a test of the LIST page itself.
    assert_eq!(
        s3.list_prefix("", None, 10, &mut visit),
        Err(BlobError::Protocol("not a ListBucketResult"))
    );
    // LIST: a test of the LIST page itself.
    assert_eq!(
        s3.list_prefix("", None, 10, &mut visit),
        Err(BlobError::Unavailable)
    );
    // LIST: a test of the LIST page itself.
    assert_eq!(
        s3.list_prefix("", None, 10, &mut visit),
        Err(BlobError::Unavailable)
    );
    // LIST: a test of the LIST page itself.
    assert_eq!(
        s3.list_prefix("", None, 10, &mut visit),
        Err(BlobError::Protocol("LIST entry without a Key or a Size"))
    );
    assert_eq!(keys, ["a"]);
    t.join().unwrap();
    assert_eq!(s3.meter().count(S3Op::List), 4);
}

#[test]
fn a_head_without_a_length_is_a_protocol_error() {
    let (s3, t) = canned(vec![b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n"]);
    assert_eq!(s3.head("k"), Err(BlobError::Protocol("no Content-Length")));
    t.join().unwrap();
}
