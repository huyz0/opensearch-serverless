//! Batch deletes: `POST /bucket?delete` (S3's `DeleteObjects`), up to
//! [`MAX_DELETE_KEYS`] keys a request, in quiet mode so the answer lists
//! only the keys that failed.
//!
//! The body is built whole, since S3 requires its `Content-MD5` before the
//! first byte: at most a thousand keys, each escaped, about 6 MiB at the
//! very worst and some tens of KiB for the keys this system names. The
//! answer is scanned as it streams for the first `<Error>`, so a store that
//! ignores quiet mode and lists every deleted key costs no memory for it.
//! S3 does not bill a DELETE, and counts a batch delete as one: each
//! request is recorded as one [`S3Op::Delete`].

use std::io::Read;

use md5::{Digest, Md5};
use ureq::http;

use super::S3Store;
use super::body::Body;
use super::request::{ERROR_BODY, failure};
use super::xml::error_code;
use crate::chunk::SliceSource;
use crate::error::{BlobError, BlobResult};
use crate::op::S3Op;
use crate::store::{MAX_DELETE_KEYS, check_key};

/// Bytes read from the answer at a time.
const ANSWER_CHUNK: usize = 4096;
/// The body's buffer as `ureq` reads it.
const BODY_CHUNK: usize = 16 * 1024;

impl S3Store {
    /// One batch delete for each [`MAX_DELETE_KEYS`] keys, in order, after
    /// checking every key; each retried as a whole, which is safe since
    /// deleting is.
    pub(super) fn delete_batches(&self, keys: &[&str]) -> BlobResult<()> {
        for key in keys {
            check_key(key)?;
        }
        for batch in keys.chunks(MAX_DELETE_KEYS) {
            let body = request_body(batch);
            let md5 = base64(&Md5::digest(&body));
            self.run(|_| Some((self.delete_batch_once(&body, &md5), true)))?;
        }
        Ok(())
    }

    fn delete_batch_once(&self, xml: &[u8], md5: &str) -> BlobResult<()> {
        // ALLOC: one number.
        let length = xml.len().to_string();
        let extra = [
            ("content-length", length.as_str()),
            ("content-md5", md5),
            ("content-type", "application/xml"),
        ];
        let mut source = SliceSource::new(xml);
        let mut buf = [0u8; BODY_CHUNK];
        let mut body = Body {
            source: &mut source,
            buf: &mut buf,
            filled: 0,
            pos: 0,
            sent: 0,
            len: u64::try_from(xml.len()).unwrap_or(u64::MAX),
            failure: None,
        };
        let mut response = self.send(
            S3Op::Delete,
            http::Method::POST,
            None,
            &[("delete", "")],
            &extra,
            Some(&mut body),
        )?;
        if response.status().as_u16() != 200 {
            return Err(failure(&mut response));
        }
        match first_error(&mut response.body_mut().as_reader()) {
            Ok(None) => Ok(()),
            Ok(Some(err)) | Err(err) => Err(err),
        }
    }
}

/// The request body for `keys`, quiet.
fn request_body(keys: &[&str]) -> Vec<u8> {
    // No namespace: the element is read by name.
    const OPEN: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Delete><Quiet>true</Quiet>";
    const CLOSE: &str = "</Delete>";
    let escaped: usize = keys
        .iter()
        .map(|k| k.len().saturating_mul(6).saturating_add(30))
        .fold(0, usize::saturating_add);
    // ALLOC: one batch's keys, at most MAX_DELETE_KEYS, each escaped.
    let mut xml = String::with_capacity(
        OPEN.len()
            .saturating_add(escaped)
            .saturating_add(CLOSE.len()),
    );
    xml.push_str(OPEN);
    for key in keys {
        xml.push_str("<Object><Key>");
        escape(key, &mut xml);
        xml.push_str("</Key></Object>");
    }
    xml.push_str(CLOSE);
    xml.into_bytes()
}

/// The failure of the first key the answer lists as `<Error>`, reading it
/// as it streams; `None` when it lists none.
fn first_error(answer: &mut dyn Read) -> BlobResult<Option<BlobError>> {
    const OPEN: &[u8] = b"<Error>";
    // The bytes of the last chunk an `<Error>` split across chunks may
    // start in, kept ahead of the next.
    let keep = OPEN.len().saturating_sub(1);
    let mut scan = [0u8; ANSWER_CHUNK + OPEN.len()];
    let mut carried = 0usize;
    loop {
        let room = scan.get_mut(carried..).unwrap_or_default();
        let n = answer.read(room).map_err(|_| BlobError::Unavailable)?;
        if n == 0 {
            return Ok(None);
        }
        let filled = carried.saturating_add(n);
        let window = scan.get(..filled).unwrap_or_default();
        if let Some(at) = window.windows(OPEN.len()).position(|w| w == OPEN) {
            let mut error = [0u8; ERROR_BODY];
            let found = window.get(at..).unwrap_or_default();
            let mut len = found.len().min(ERROR_BODY);
            if let (Some(dst), Some(src)) = (error.get_mut(..len), found.get(..len)) {
                dst.copy_from_slice(src);
            }
            while let Some(rest) = error.get_mut(len..).filter(|r| !r.is_empty()) {
                match answer.read(rest) {
                    Ok(m) if m > 0 => len = len.saturating_add(m),
                    _ => break,
                }
            }
            let code = error_code(error.get(..len).unwrap_or_default());
            return Ok(Some(key_failure(code)));
        }
        carried = keep.min(filled);
        scan.copy_within(filled.saturating_sub(carried)..filled, 0);
    }
}

/// What a key's error code means: retried unless it is a refusal.
fn key_failure(code: Option<&str>) -> BlobError {
    match code {
        Some("AccessDenied") => BlobError::Denied,
        Some("SlowDown") => BlobError::Throttled,
        _ => BlobError::Unavailable,
    }
}

/// Appends `text` with the five XML specials escaped.
pub(crate) fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
}

/// Standard base64 with padding, as `Content-MD5` wants it.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    // ALLOC: one digest's text, 24 bytes.
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for group in bytes.chunks(3) {
        let b = [
            group.first().copied().unwrap_or(0),
            group.get(1).copied().unwrap_or(0),
            group.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4usize {
            if i <= group.len() {
                let shift = 18usize.saturating_sub(i.saturating_mul(6));
                let index = usize::try_from((n >> shift) & 63).unwrap_or(0);
                out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), want, "{input}");
        }
        // RFC 1864's own example digest is of no fixed text, so check the
        // MD5 of the empty body S3's docs quote.
        assert_eq!(base64(&Md5::digest(b"")), "1B2M2Y8AsgTpgAmY7PhCfg==");
    }

    #[test]
    fn the_body_names_each_key_escaped_and_asks_for_quiet() {
        let body = String::from_utf8(request_body(&["a&b", "c<d>"])).unwrap();
        assert!(body.contains("<Quiet>true</Quiet>"));
        assert!(
            body.contains(
                "<Object><Key>a&amp;b</Key></Object><Object><Key>c&lt;d&gt;</Key></Object>"
            )
        );
        assert!(body.ends_with("</Delete>"));
    }

    fn scan(answer: &[u8], piece: usize) -> BlobResult<Option<BlobError>> {
        struct Pieces<'a>(&'a [u8], usize);
        impl Read for Pieces<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let n = self.1.min(out.len()).min(self.0.len());
                out[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        first_error(&mut Pieces(answer, piece))
    }

    #[test]
    fn an_answer_listing_no_error_is_success_however_it_arrives() {
        let mut answer = b"<?xml version=\"1.0\"?><DeleteResult>".to_vec();
        for i in 0..2_000 {
            answer.extend_from_slice(format!("<Deleted><Key>k{i}</Key></Deleted>").as_bytes());
        }
        answer.extend_from_slice(b"</DeleteResult>");
        for piece in [1, 3, 7, 4096] {
            assert_eq!(scan(&answer, piece), Ok(None), "{piece}");
        }
    }

    #[test]
    fn the_first_error_listed_is_the_failure_wherever_it_falls() {
        for (code, want) in [
            ("AccessDenied", BlobError::Denied),
            ("SlowDown", BlobError::Throttled),
            ("InternalError", BlobError::Unavailable),
        ] {
            let mut answer = b"<DeleteResult>".to_vec();
            for i in 0..500 {
                answer.extend_from_slice(format!("<Deleted><Key>k{i}</Key></Deleted>").as_bytes());
            }
            answer.extend_from_slice(
                format!("<Error><Key>x</Key><Code>{code}</Code><Message>m</Message></Error>")
                    .as_bytes(),
            );
            answer.extend_from_slice(b"</DeleteResult>");
            for piece in [1, 5, 6, 7, 4096, 1 << 20] {
                assert_eq!(
                    scan(&answer, piece),
                    Ok(Some(want.clone())),
                    "{code} {piece}"
                );
            }
        }
        assert_eq!(
            scan(b"<DeleteResult><Error><Key>x</Key>", 2),
            Ok(Some(BlobError::Unavailable)),
            "an error with no code"
        );
    }

    #[test]
    fn an_answer_that_breaks_off_is_unavailable() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("reset"))
            }
        }
        assert_eq!(first_error(&mut Broken), Err(BlobError::Unavailable));
    }
}
