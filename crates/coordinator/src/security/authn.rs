//! Reading credentials, and the `authinfo` answer.

use oss_doc::mapping::json::write_str;

use super::config::Config;

/// The most bytes of an `Authorization` header read; a longer one is no
/// credentials.
pub const MAX_HEADER_BYTES: usize = 1024;

// ARITH: bytes inside the matched ranges, so each subtraction is non-negative.
#[allow(clippy::arithmetic_side_effects)]
fn sextet(b: u8) -> Option<u32> {
    match b {
        b'A'..=b'Z' => Some(u32::from(b - b'A')),
        b'a'..=b'z' => Some(u32::from(b - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(b - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decodes standard base64, with or without padding.
// ARITH: at most 1 KiB of input; `acc` keeps 6 bits a byte and `bits` stays under 14.
#[allow(clippy::arithmetic_side_effects)]
fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.trim_end_matches('=').as_bytes();
    // ALLOC: the decoded header, at most MAX_HEADER_BYTES.
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = ((acc << 6) | sextet(b)?) & 0xffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// The user and password of a `Basic` header, or `None` for anything else:
/// another scheme, a header over [`MAX_HEADER_BYTES`], bad base64, text
/// that is not UTF-8 or has no `:`.
pub fn basic(header: &str) -> Option<(String, String)> {
    if header.len() > MAX_HEADER_BYTES {
        return None;
    }
    let (scheme, rest) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(decode(rest.trim())?).ok()?;
    let (user, password) = decoded.split_once(':')?;
    if user.is_empty() {
        return None;
    }
    // ALLOC: one answer a request, bounded by the 1 KiB header or the stored document.
    Some((user.to_owned(), password.to_owned()))
}

/// The body `GET /_plugins/_security/authinfo` answers for `name`.
pub fn authinfo(config: &Config, name: &str) -> String {
    let user = config.user(name);
    let backend = user.map(|u| u.backend_roles.as_slice()).unwrap_or_default();
    // ALLOC: one answer a request.
    let mut out = String::new();
    let mut label = String::new();
    write_str(
        &mut label,
        // ALLOC: one answer a request, bounded by the 1 KiB header or the stored document.
        &format!(
            "User [name={name}, backend_roles=[{}], requestedTenant=null]",
            backend.join(", ")
        ),
    );
    out.push_str("{\"user\":");
    out.push_str(&label);
    out.push_str(",\"user_name\":");
    write_str(&mut out, name);
    out.push_str(",\"user_requested_tenant\":null,\"remote_address\":null,\"backend_roles\":");
    array(&mut out, backend.iter().map(String::as_str));
    out.push_str(",\"custom_attribute_names\":");
    let attributes: Vec<String> = user
        .map(|u| {
            u.attributes
                .iter()
                // ALLOC: one answer a request, bounded by the 1 KiB header or the stored document.
                .map(|a| format!("attr.internal.{a}"))
                // ALLOC: one answer a request, bounded by the 1 KiB header or the stored document.
                .collect()
        })
        .unwrap_or_default();
    array(&mut out, attributes.iter().map(String::as_str));
    out.push_str(",\"roles\":");
    let roles = config.roles_of(name);
    array(&mut out, roles.iter().map(String::as_str));
    out.push_str(",\"tenants\":{");
    for (i, t) in config.tenants_of(name).iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(&mut out, t);
        out.push_str(":true");
    }
    out.push_str("},\"principal\":null,\"peer_certificates\":\"0\",\"sso_logout_url\":null}");
    out
}

fn array<'a>(out: &mut String, items: impl Iterator<Item = &'a str>) {
    out.push('[');
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(out, item);
    }
    out.push(']');
}
