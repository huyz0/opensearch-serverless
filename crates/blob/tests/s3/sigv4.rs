//! The signer against AWS's published SigV4 test suite (AC-1).

use std::path::{Path, PathBuf};

use oss_blob::s3::sigv4::{self, Credentials, Scope};

fn vectors() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sigv4");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn file(dir: &Path, ext: &str) -> String {
    let name = dir.file_name().unwrap().to_str().unwrap();
    #[allow(clippy::disallowed_methods)]
    std::fs::read_to_string(dir.join(format!("{name}.{ext}"))).unwrap()
}

fn unhex(d: u8) -> u8 {
    (d as char).to_digit(16).unwrap() as u8
}

/// Percent-decodes `s`, as a server reads the bytes on the wire.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(unhex(b[i.wrapping_add(1)]).wrapping_shl(4) | unhex(b[i.wrapping_add(2)]));
            i = i.wrapping_add(3);
        } else {
            out.push(b[i]);
            i = i.wrapping_add(1);
        }
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn every_published_vector_signs_exactly() {
    let all = vectors();
    assert_eq!(all.len(), 21, "vectors under tests/sigv4");
    for dir in all {
        // The generic suite signs with its example key for service
        // `service`; the S3 example names its own key, region and service.
        #[allow(clippy::disallowed_methods)]
        let scope_file = std::fs::read_to_string(dir.join("scope.txt")).unwrap_or_else(|_| {
            "AKIDEXAMPLE\nwJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY\nus-east-1\nservice\n".into()
        });
        let s: Vec<&str> = scope_file.lines().collect();
        let creds = Credentials::new(s[0], s[1]);
        let req = file(&dir, "req");
        let (head, body) = req.split_once("\n\n").unwrap_or((&req, ""));
        let mut lines = head.lines();
        let request_line = lines.next().unwrap();
        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap();
        let target = parts.next().unwrap();
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let headers: Vec<(&str, &str)> = lines.map(|l| l.split_once(':').unwrap()).collect();
        let pairs: Vec<(String, String)> = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (n, v) = p.split_once('=').unwrap_or((p, ""));
                (decode(n), decode(v))
            })
            .collect();
        let pairs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let mut uri = String::new();
        sigv4::uri_encode(decode(path).as_bytes(), true, &mut uri);
        let canonical = sigv4::canonical_request(
            method,
            &uri,
            &sigv4::canonical_query(&pairs),
            &headers,
            &sigv4::sha256_hex(body.as_bytes()),
        );
        let name = dir.display();
        let published = |ext: &str| {
            dir.join(format!(
                "{}.{ext}",
                dir.file_name().unwrap().to_str().unwrap()
            ))
            .is_file()
        };
        if published("creq") {
            assert_eq!(
                canonical.request,
                file(&dir, "creq"),
                "{name}: canonical request"
            );
        }
        let date = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("x-amz-date"))
            .unwrap()
            .1;
        let scope = Scope {
            date: &date[..8],
            region: s[2],
            service: s[3],
        };
        if published("sts") {
            assert_eq!(
                sigv4::string_to_sign(date, &scope, &canonical.request),
                file(&dir, "sts"),
                "{name}: string to sign"
            );
        }
        assert_eq!(
            sigv4::authorization(&creds, &scope, date, &canonical),
            file(&dir, "authz"),
            "{name}: authorization"
        );
    }
}
