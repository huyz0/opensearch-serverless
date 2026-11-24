//! Tests; compiled only under `#[cfg(test)]` from the parent module.

#[cfg(test)]
use std::io::Write;

use super::*;
use crate::error::RestError;
use crate::method::{Method, MethodSet};

fn with<F: FnOnce(&mut Response<'_>)>(buf_len: usize, head_only: bool, f: F) -> String {
    let mut out = Vec::new();
    let mut buf = vec![0u8; buf_len];
    let mut resp = Response::new(
        &mut out,
        &mut buf,
        head_only,
        Some("close"),
        Some("op-1"),
        None,
    );
    f(&mut resp);
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn a_known_length_body_goes_out_with_its_head() {
    let text = with(256, false, |r| {
        assert!(format!("{r:?}").contains("NotStarted"));
        r.send(200, JSON, b"{\"a\":1}").expect("sends");
        assert!(r.is_started() && r.is_finished());
    });
    assert_eq!(
        text,
        "HTTP/1.1 200 OK\r\nx-opaque-id: op-1\r\ncontent-type: application/json; charset=UTF-8\r\ncontent-length: 7\r\nconnection: close\r\n\r\n{\"a\":1}"
    );
}

#[test]
fn a_chunked_body_is_sent_one_buffer_per_chunk() {
    let text = with(256, false, |r| {
        let mut w = r
            .start(200, Some(JSON), Length::Chunked, &[("x-a", "b")])
            .expect("starts");
        for _ in 0..30 {
            w.write_all(b"0123456789").expect("writes");
        }
        w.finish().expect("finishes");
    });
    let (head, body) = text.split_once("\r\n\r\n").expect("head");
    assert!(head.contains("transfer-encoding: chunked") && head.contains("x-a: b"));
    assert!(!head.contains("content-length"));
    // Decode the chunks: none is larger than the buffer, and they join to
    // the body written.
    let mut rest = body;
    let mut joined = String::new();
    loop {
        let (size, after) = rest.split_once("\r\n").expect("size line");
        let n = usize::from_str_radix(size, 16).expect("hex");
        if n == 0 {
            assert_eq!(after, "\r\n");
            break;
        }
        assert!(n <= 256, "a chunk is at most one buffer");
        joined.push_str(&after[..n]);
        rest = after[n..].strip_prefix("\r\n").expect("chunk end");
    }
    assert_eq!(joined, "0123456789".repeat(30));
}

#[test]
fn an_explicit_flush_sends_a_chunk_now() {
    let text = with(256, false, |r| {
        let mut w = r.start(200, None, Length::Chunked, &[]).expect("starts");
        w.write_all(b"abc").expect("writes");
        w.flush().expect("flushes");
        w.write_all(b"de").expect("writes");
        w.finish().expect("finishes");
    });
    assert!(
        text.ends_with("\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"),
        "{text}"
    );
}

#[test]
fn head_responses_keep_the_length_and_drop_the_body() {
    let text = with(256, true, |r| {
        r.send(200, JSON, b"{\"a\":1}").expect("sends")
    });
    assert!(
        text.ends_with("content-length: 7\r\nconnection: close\r\n\r\n"),
        "{text}"
    );
    let text = with(256, true, |r| {
        let mut w = r.start(200, None, Length::Chunked, &[]).expect("starts");
        w.write_all(b"abc").expect("writes");
        w.finish().expect("finishes");
    });
    assert!(
        text.ends_with("transfer-encoding: chunked\r\nconnection: close\r\n\r\n"),
        "{text}"
    );
}

#[test]
fn a_known_length_is_enforced_both_ways() {
    with(256, false, |r| {
        let mut w = r.start(200, None, Length::Known(3), &[]).expect("starts");
        assert!(w.write_all(b"abcd").is_err(), "longer than declared");
        w.write_all(b"ab").expect("fits");
        assert!(w.finish().is_err(), "shorter than declared");
        assert!(!r.is_finished());
        assert!(
            r.start(200, None, Length::Known(0), &[]).is_err(),
            "already started"
        );
    });
}

#[test]
fn errors_carry_their_status_length_and_allow_header() {
    let mut allowed = MethodSet::EMPTY;
    allowed.insert(Method::Get);
    allowed.insert(Method::Post);
    let e = RestError::method_not_allowed("/_search", Some(Method::Delete), allowed);
    let text = with(512, false, |r| r.send_error(&e).expect("sends"));
    let (head, body) = text.split_once("\r\n\r\n").expect("head");
    assert!(
        head.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"),
        "{head}"
    );
    assert!(head.contains("allow: GET,POST\r\n"));
    assert!(head.contains(&format!("content-length: {}\r\n", body.len())));
    assert!(head.contains("content-type: application/json; charset=UTF-8"));

    let text = with(512, false, |r| {
        r.send_error(&RestError::too_large()).expect("sends")
    });
    assert!(text.starts_with("HTTP/1.1 413 Request Entity Too Large\r\n"));
    assert!(text.contains("content-length: 0\r\n") && !text.contains("content-type"));
}

#[test]
fn a_head_larger_than_the_buffer_is_an_error() {
    with(16, false, |r| {
        assert!(r.start(200, Some(JSON), Length::Known(0), &[]).is_err());
    });
}

#[test]
fn reason_phrases_cover_the_statuses_the_edge_sends() {
    for (code, phrase) in [
        (100, "Continue"),
        (201, "Created"),
        (202, "Accepted"),
        (204, "No Content"),
        (401, "Unauthorized"),
        (403, "Forbidden"),
        (404, "Not Found"),
        (406, "Not Acceptable"),
        (408, "Request Timeout"),
        (409, "Conflict"),
        (412, "Precondition Failed"),
        (415, "Unsupported Media Type"),
        (417, "Expectation Failed"),
        (429, "Too Many Requests"),
        (500, "Internal Server Error"),
        (501, "Not Implemented"),
        (503, "Service Unavailable"),
        (299, "Unknown"),
    ] {
        assert_eq!(reason_phrase(code), phrase);
    }
}

#[test]
fn fixed_strings_refuse_past_their_size() {
    use std::fmt::Write as _;
    let mut s = FixedStr::<4>::default();
    s.write_str("abcd").expect("fits");
    assert!(s.write_str("e").is_err());
    assert_eq!(s.as_str(), "abcd");
}

fn response<'a>(
    out: &'a mut Vec<u8>,
    buf: &'a mut [u8],
    connection: Option<&'static str>,
) -> Response<'a> {
    Response::new(out, buf, false, connection, None, None)
}

#[test]
fn a_response_closes_the_connection_only_when_it_says_close() {
    let (mut out, mut buf) = (Vec::new(), [0u8; 64]);
    assert!(response(&mut out, &mut buf, Some("close")).closes());
    assert!(!response(&mut out, &mut buf, Some("keep-alive")).closes());
    assert!(!response(&mut out, &mut buf, None).closes());
}

#[test]
fn a_known_length_head_waits_for_its_body_but_an_empty_one_goes_out_at_once() {
    let (mut out, mut buf) = (Vec::new(), [0u8; 256]);
    {
        let mut r = response(&mut out, &mut buf, None);
        r.start(200, None, Length::Known(5), &[]).expect("starts");
    }
    assert!(out.is_empty(), "the head is held until the body is written");
    {
        let mut r = response(&mut out, &mut buf, None);
        r.start(200, None, Length::Known(0), &[]).expect("starts");
    }
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 200 OK\r\n"));
}

#[test]
fn a_bad_request_has_its_reason_phrase() {
    assert_eq!(reason_phrase(400), "Bad Request");
    let text = with(256, false, |r| {
        r.send(400, JSON, b"{}").expect("sends");
    });
    assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{text}");
}
