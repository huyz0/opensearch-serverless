//! A node over HTTP for the replay tests: one connection a request.

// ARITH: reply offsets and sizes are small; an overflow panics the test,
// which fails it as it should.
#![allow(dead_code, clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use oss_compat::pointer::Pointer;
use oss_compat::{Fixture, Request, Response, Rule, Target, TargetError};

/// A node over HTTP, one connection a request.
pub struct HttpTarget {
    pub addr: SocketAddr,
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Target for HttpTarget {
    fn send(&mut self, request: &Request) -> Result<Response, TargetError> {
        let failed = |e: std::io::Error| TargetError {
            message: e.to_string(),
        };
        let mut target = request.path.clone();
        for (i, (k, v)) in request.query.iter().enumerate() {
            target.push(if i == 0 { '?' } else { '&' });
            target.push_str(&format!("{}={}", encode(k), encode(v)));
        }
        let body = request.encode_body();
        let mut head = format!(
            "{} {target} HTTP/1.1\r\nhost: test\r\nconnection: close\r\n",
            request.method.as_str()
        );
        if let (Some(b), Some(ct)) = (&body, request.content_type()) {
            head.push_str(&format!(
                "content-type: {ct}\r\ncontent-length: {}\r\n",
                b.len()
            ));
        }
        head.push_str("\r\n");
        let mut s = TcpStream::connect(self.addr).map_err(failed)?;
        s.set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(failed)?;
        s.write_all(head.as_bytes()).map_err(failed)?;
        if let Some(b) = &body {
            s.write_all(b).map_err(failed)?;
        }
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).map_err(failed)?;
        Ok(parse(&raw))
    }
}

fn parse(raw: &[u8]) -> Response {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    let head = std::str::from_utf8(&raw[..end]).expect("utf-8 head");
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .expect("a status");
    let headers: std::collections::BTreeMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let mut rest = &raw[end + 4..];
    let body = if headers.get("transfer-encoding").map(String::as_str) == Some("chunked") {
        let mut out = Vec::new();
        loop {
            let line = rest
                .windows(2)
                .position(|w| w == b"\r\n")
                .expect("chunk size");
            let size =
                usize::from_str_radix(std::str::from_utf8(&rest[..line]).expect("utf-8"), 16)
                    .expect("hex size");
            rest = &rest[line + 2..];
            if size == 0 {
                break out;
            }
            out.extend_from_slice(&rest[..size]);
            rest = &rest[size + 2..];
        }
    } else {
        rest.to_vec()
    };
    let body = (!body.is_empty()).then(|| {
        serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("json body ({e}): {}", String::from_utf8_lossy(&body)))
    });
    Response {
        status,
        headers,
        body,
    }
}

/// The steps whose error names a column of the body. The fixture holds the
/// body as a JSON value, whose keys `oss-compat` keeps sorted, so the bytes
/// replayed are not the bytes recorded and the column moves. The reader's
/// own tests check these columns on the recorded bytes
/// (`dsl::source_tests` and `dsl::query_tests`).
const COLUMN_STEPS: &[(&str, &str)] = &[
    ("search errors", "unknown top-level key"),
    ("search errors", "unknown term parameter"),
];

/// Adds the column rules to the steps whose error names a column.
pub fn normalise_columns(f: &mut Fixture) {
    for step in &mut f.steps {
        let function_score = f.name == "match queries"
            && step.name.starts_with("fs ")
            && step
                .response
                .as_ref()
                .and_then(|r| r.body.as_ref())
                .is_some_and(|b| b.pointer("/error/col").is_some());
        if function_score || COLUMN_STEPS.contains(&(f.name.as_str(), step.name.as_str())) {
            for at in ["/body/error/col", "/body/error/root_cause/0/col"] {
                step.normalise.push(Rule {
                    pointer: Pointer::parse(at).expect("pointer"),
                    reason: "the replayed body's keys are re-sorted".into(),
                });
            }
        }
    }
}
