//! AWS Signature Version 4, header form, for the requests [`S3Store`]
//! sends.
//!
//! The Java shell signs through the AWS SDK; there is no Java source of our
//! own to port. The steps follow AWS's published algorithm: a canonical
//! request, a string to sign over its SHA-256, and an HMAC chain from the
//! secret to a signing key. Everything here is a pure function of its
//! arguments (the caller supplies the clock), so the AWS test suite under
//! `crates/blob/tests/sigv4/` checks it byte for byte, and the emulator's
//! HTTP front end uses the same functions to verify what arrives on the
//! wire.
//!
//! [`S3Store`]: super::S3Store

use std::fmt::{self, Write as _};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// The algorithm name every header and string to sign carries.
pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The payload hash for a body that is not hashed: it streams in one pass,
/// and TLS protects it (ADR 0005).
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// An access key and its secret. `Debug` shows neither.
#[derive(Clone)]
pub struct Credentials {
    access_key: String,
    secret_key: String,
}

impl Credentials {
    /// Credentials from an access key id and its secret.
    pub fn new(access_key: &str, secret_key: &str) -> Self {
        Self {
            // ALLOC: once per store, at construction.
            access_key: access_key.to_owned(),
            // ALLOC: once per store, at construction.
            secret_key: secret_key.to_owned(),
        }
    }

    /// The access key id, which travels in the clear in every request.
    pub fn access_key(&self) -> &str {
        &self.access_key
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials").finish_non_exhaustive()
    }
}

/// Lowercase hex of `bytes`, appended to `out`.
pub fn hex_into(bytes: &[u8], out: &mut String) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
}

/// Lowercase hex of the SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    // ALLOC: 64 bytes.
    let mut out = String::with_capacity(64);
    hex_into(&Sha256::digest(data), &mut out);
    out
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    // HMAC takes a key of any length, so this cannot fail.
    let mut mac =
        <HmacSha256 as KeyInit>::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Percent-encodes `bytes` as SigV4 and S3 require, appending to `out`:
/// every byte except `A-Z a-z 0-9 - _ . ~` becomes `%XX` in uppercase hex,
/// and `/` is kept when `keep_slash` (an object key's path) or encoded
/// (a query name or value).
pub fn uri_encode(bytes: &[u8], keep_slash: bool, out: &mut String) {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    for &b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            b'/' if keep_slash => out.push('/'),
            _ => {
                out.push('%');
                out.push(char::from(DIGITS[usize::from(b >> 4)]));
                out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
            }
        }
    }
}

/// The canonical query string: each name and value encoded, sorted by
/// encoded name and then value, joined as `name=value` with `&`. `pairs`
/// are decoded; a name without a value is passed with an empty value.
pub fn canonical_query(pairs: &[(&str, &str)]) -> String {
    // ALLOC: one entry per query parameter; a request carries at most four.
    let mut encoded: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| {
            // ALLOC: three bytes per input byte at most.
            let mut n = String::with_capacity(name.len().saturating_mul(3));
            uri_encode(name.as_bytes(), false, &mut n);
            // ALLOC: three bytes per input byte at most.
            let mut v = String::with_capacity(value.len().saturating_mul(3));
            uri_encode(value.as_bytes(), false, &mut v);
            (n, v)
        })
        // ALLOC: as above, at most four entries.
        .collect();
    encoded.sort();
    // ALLOC: the encoded pairs plus separators.
    let mut out = String::with_capacity(encoded.iter().fold(0usize, |s, (n, v)| {
        s.saturating_add(n.len())
            .saturating_add(v.len())
            .saturating_add(2)
    }));
    for (i, (n, v)) in encoded.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(n);
        out.push('=');
        out.push_str(v);
    }
    out
}

/// Appends a header value with its ends trimmed and each inner run of
/// spaces and tabs collapsed to one space.
fn push_trimmed(value: &str, out: &mut String) {
    let mut space = false;
    for c in value.trim_matches([' ', '\t']).chars() {
        if c == ' ' || c == '\t' {
            space = true;
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
}

/// A canonical request and the `SignedHeaders` list it signs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canonical {
    /// The canonical request, hashed into the string to sign.
    pub request: String,
    /// The signed header names, lowercase, sorted, joined with `;`.
    pub signed_headers: String,
}

/// Builds the canonical request. `canonical_uri` is the encoded path and
/// `canonical_query` comes from [`canonical_query`]. `headers` are every
/// header to sign, in any case and order; repeated names are joined with
/// `,` in the order given, as SigV4 specifies.
pub fn canonical_request(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &[(&str, &str)],
    payload_hash: &str,
) -> Canonical {
    // ALLOC: one entry per signed header; a request signs at most a dozen.
    let mut sorted: Vec<(String, &str)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), *value))
        // ALLOC: as above, one lowercased name per signed header.
        .collect();
    // Stable, so repeated names keep the order they were given in.
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let size = headers.iter().fold(0usize, |s, (n, v)| {
        s.saturating_add(n.len())
            .saturating_add(v.len())
            .saturating_add(2)
    });
    // ALLOC: the request line, the signed headers twice, and the hash.
    let mut request = String::with_capacity(
        method
            .len()
            .saturating_add(canonical_uri.len())
            .saturating_add(canonical_query.len())
            .saturating_add(size.saturating_mul(2))
            .saturating_add(payload_hash.len())
            .saturating_add(8),
    );
    // ALLOC: the header names and separators.
    let mut signed_headers = String::with_capacity(size);
    request.push_str(method);
    request.push('\n');
    request.push_str(canonical_uri);
    request.push('\n');
    request.push_str(canonical_query);
    request.push('\n');
    let mut previous: Option<&str> = None;
    for (name, value) in &sorted {
        if previous == Some(name.as_str()) {
            request.push(',');
        } else {
            if previous.is_some() {
                request.push('\n');
                signed_headers.push(';');
            }
            request.push_str(name);
            request.push(':');
            signed_headers.push_str(name);
        }
        push_trimmed(value, &mut request);
        previous = Some(name.as_str());
    }
    if previous.is_some() {
        request.push('\n');
    }
    request.push('\n');
    request.push_str(&signed_headers);
    request.push('\n');
    request.push_str(payload_hash);
    Canonical {
        request,
        signed_headers,
    }
}

/// Where a signature is valid: a day, a region and a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope<'a> {
    /// `YYYYMMDD`, the first eight characters of the request's `x-amz-date`.
    pub date: &'a str,
    /// The region, such as `us-east-1`.
    pub region: &'a str,
    /// The service, `s3` for every request this crate sends.
    pub service: &'a str,
}

impl Scope<'_> {
    /// `date/region/service/aws4_request`.
    pub fn render(&self) -> String {
        // ALLOC: the four scope parts and separators.
        format!(
            "{}/{}/{}/aws4_request",
            self.date, self.region, self.service
        )
    }
}

/// The string to sign for a canonical request made at `amz_date`
/// (`YYYYMMDDTHHMMSSZ`).
pub fn string_to_sign(amz_date: &str, scope: &Scope<'_>, canonical_request: &str) -> String {
    // ALLOC: the algorithm, the date, the scope and a 64-byte hash.
    format!(
        "{ALGORITHM}\n{amz_date}\n{}\n{}",
        scope.render(),
        sha256_hex(canonical_request.as_bytes())
    )
}

/// The signature of `string_to_sign`, in lowercase hex, under the key
/// derived from `secret_key` and `scope`.
pub fn signature(secret_key: &str, scope: &Scope<'_>, string_to_sign: &str) -> String {
    // ALLOC: the secret with a four-byte prefix.
    let mut seed = String::with_capacity(secret_key.len().saturating_add(4));
    seed.push_str("AWS4");
    seed.push_str(secret_key);
    let k_date = hmac(seed.as_bytes(), scope.date.as_bytes());
    let k_region = hmac(&k_date, scope.region.as_bytes());
    let k_service = hmac(&k_region, scope.service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    // ALLOC: 64 bytes.
    let mut out = String::with_capacity(64);
    hex_into(&hmac(&k_signing, string_to_sign.as_bytes()), &mut out);
    out
}

/// The `Authorization` header value for `canonical`, made at `amz_date`.
pub fn authorization(
    credentials: &Credentials,
    scope: &Scope<'_>,
    amz_date: &str,
    canonical: &Canonical,
) -> String {
    let to_sign = string_to_sign(amz_date, scope, &canonical.request);
    let sig = signature(&credentials.secret_key, scope, &to_sign);
    // ALLOC: the key id, the scope, the signed header list and 64 bytes.
    let mut out = String::with_capacity(
        credentials
            .access_key
            .len()
            .saturating_add(canonical.signed_headers.len())
            .saturating_add(200),
    );
    // Writing to a String cannot fail.
    let _ = write!(
        out,
        "{ALGORITHM} Credential={}/{}, SignedHeaders={}, Signature={sig}",
        credentials.access_key,
        scope.render(),
        canonical.signed_headers
    );
    out
}

/// `YYYYMMDDTHHMMSSZ` for `unix_seconds` since the epoch, in UTC.
pub fn amz_date(unix_seconds: u64) -> String {
    let days = unix_seconds / 86_400;
    let secs = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    // ALLOC: 16 bytes.
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01, by Howard
/// Hinnant's `civil_from_days`, over unsigned values: every input from the
/// epoch on is in range, and a year past `u64::MAX / 365` cannot occur.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // 719_468 shifts the epoch to 0000-03-01; days is a Unix day count, so
    // the sum is far below u64::MAX. Every other operation is a division, a
    // remainder, or a product and difference of values already reduced below
    // 146_097 by the remainder, and doe >= doe/1460 - doe/36524 + doe/146096.
    // ARITH: no operation below can overflow or go negative, as above.
    #[allow(clippy::arithmetic_side_effects)]
    {
        let z = days + 719_468;
        let era = z / 146_097;
        let doe = z % 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + u64::from(month <= 2);
        (year, month, day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_render_as_amz_dates() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_440_938_160), "20150830T123600Z");
        // A leap day, and the end of a century that is a leap year.
        assert_eq!(amz_date(951_782_400), "20000229T000000Z");
        assert_eq!(amz_date(4_102_444_799), "20991231T235959Z");
        // Past a century without a leap day, and the last day of an era.
        assert_eq!(amz_date(4_107_456_000), "21000228T000000Z");
        assert_eq!(amz_date(4_107_542_400), "21000301T000000Z");
        assert_eq!(amz_date(13_569_379_200), "23991231T000000Z");
        assert_eq!(amz_date(13_574_563_200), "24000229T000000Z");
    }

    #[test]
    fn debug_output_hides_the_secret() {
        let c = Credentials::new("AKIDEXAMPLE", "very-secret");
        let shown = format!("{c:?}");
        assert_eq!(shown, "Credentials { .. }");
        assert!(!shown.contains("very-secret"));
    }

    #[test]
    fn encoding_keeps_unreserved_bytes_and_slashes_only_in_paths() {
        let mut out = String::new();
        uri_encode(b"a-Z_0.~/ +=&", true, &mut out);
        assert_eq!(out, "a-Z_0.~/%20%2B%3D%26");
        out.clear();
        uri_encode("é/".as_bytes(), false, &mut out);
        assert_eq!(out, "%C3%A9%2F");
    }

    #[test]
    fn the_query_is_sorted_after_encoding() {
        assert_eq!(
            canonical_query(&[("prefix", "a b"), ("list-type", "2"), ("max-keys", "")]),
            "list-type=2&max-keys=&prefix=a%20b"
        );
        assert_eq!(canonical_query(&[]), "");
    }

    #[test]
    fn header_values_are_trimmed_and_runs_collapsed() {
        let mut out = String::new();
        push_trimmed("  a \t b   c ", &mut out);
        assert_eq!(out, "a b c");
    }

    #[test]
    fn headers_are_sorted_by_name_and_repeats_kept_in_order() {
        let c = canonical_request(
            "PUT",
            "/b/k",
            "",
            &[
                ("X-Amz-Date", "d"),
                ("host", "h"),
                ("content-length", "5"),
                ("x-amz-meta-a", "2"),
                ("if-none-match", "*"),
                ("X-Amz-Meta-A", "1"),
            ],
            "p",
        );
        assert_eq!(
            c.request,
            "PUT\n/b/k\n\ncontent-length:5\nhost:h\nif-none-match:*\nx-amz-date:d\nx-amz-meta-a:2,1\n\ncontent-length;host;if-none-match;x-amz-date;x-amz-meta-a\np"
        );
    }

    #[test]
    fn a_request_without_headers_still_ends_its_header_block() {
        let c = canonical_request("GET", "/", "", &[], "h");
        assert_eq!(c.request, "GET\n/\n\n\n\nh");
        assert_eq!(c.signed_headers, "");
    }

    #[test]
    fn debug_shows_no_key() {
        let c = Credentials::new("AKID", "secret");
        let shown = format!("{c:?}");
        assert!(!shown.contains("AKID") && !shown.contains("secret"));
        assert_eq!(c.access_key(), "AKID");
    }

    #[test]
    fn an_empty_body_hashes_to_the_known_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
