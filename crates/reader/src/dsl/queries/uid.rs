//! `Uid.encodeId` (server/src/main/java/org/opensearch/index/mapper/Uid.java
//! at OpenSearch 3.10.0): the bytes an `_id` is indexed as, so an `ids`
//! query finds it. The inverse is `response::write_id`.

/// The first byte of a numeric id.
const NUMERIC: u8 = 0xfe;
/// The first byte of a UTF-8 id.
const UTF8: u8 = 0xff;
/// The byte that escapes a base64 id whose first byte would read as one of
/// the markers.
const BASE64_ESCAPE: u8 = 0xfd;

/// `Uid.isURLBase64WithoutPadding`.
fn is_url_base64_without_padding(id: &str) -> bool {
    let bytes = id.as_bytes();
    let last = bytes.last().copied().unwrap_or(b'A');
    let ok_last = match bytes.len() & 3 {
        0 => true,
        1 => false,
        2 => b"AQgw".contains(&last),
        _ => b"AEIMQUYcgkosw048".contains(&last),
    };
    ok_last
        && bytes
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
}

fn base64_value(c: u8) -> u32 {
    match c {
        b'A'..=b'Z' => u32::from(c.saturating_sub(b'A')),
        b'a'..=b'z' => u32::from(c.saturating_sub(b'a')).saturating_add(26),
        b'0'..=b'9' => u32::from(c.saturating_sub(b'0')).saturating_add(52),
        b'-' => 62,
        _ => 63,
    }
}

/// `Uid.encodeId`; `None` for an empty id ("Ids can't be empty").
pub fn encode_id(id: &str) -> Option<Vec<u8>> {
    if id.is_empty() {
        return None;
    }
    let bytes = id.as_bytes();
    if bytes.iter().all(u8::is_ascii_digit) {
        // ALLOC: one id's bytes, half its length plus two.
        let mut out = Vec::with_capacity(bytes.len().div_ceil(2).saturating_add(1));
        out.push(NUMERIC);
        for pair in bytes.chunks(2) {
            let high = pair.first().map_or(0, |b| b.saturating_sub(b'0'));
            let low = pair.get(1).map_or(0x0f, |b| b.saturating_sub(b'0'));
            out.push(high.saturating_mul(16).saturating_add(low));
        }
        return Some(out);
    }
    if is_url_base64_without_padding(id) {
        // ALLOC: one id's bytes, three quarters of its length plus two.
        let mut out = Vec::with_capacity((bytes.len().saturating_mul(3) / 4).saturating_add(2));
        for chunk in bytes.chunks(4) {
            let mut n = 0u32;
            for (i, c) in chunk.iter().enumerate() {
                let shift =
                    18u32.saturating_sub(6u32.saturating_mul(u32::try_from(i).unwrap_or(0)));
                n |= base64_value(*c) << shift;
            }
            let decoded = chunk.len().saturating_sub(1);
            for k in 0..decoded {
                let shift =
                    16u32.saturating_sub(8u32.saturating_mul(u32::try_from(k).unwrap_or(0)));
                out.push(u8::try_from((n >> shift) & 0xff).unwrap_or(0));
            }
        }
        if out.first().is_some_and(|b| *b >= BASE64_ESCAPE) {
            out.insert(0, BASE64_ESCAPE);
        }
        return Some(out);
    }
    // ALLOC: one id's bytes plus one.
    let mut out = Vec::with_capacity(bytes.len().saturating_add(1));
    out.push(UTF8);
    out.extend_from_slice(bytes);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::write_id;

    fn round_trip(id: &str) -> String {
        let mut out = Vec::new();
        write_id(&mut out, &encode_id(id).expect("not empty")).expect("writes");
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn numeric_ids_pack_two_digits_a_byte() {
        assert_eq!(encode_id("1"), Some(vec![0xfe, 0x1f]));
        assert_eq!(encode_id("12"), Some(vec![0xfe, 0x12]));
        assert_eq!(encode_id("123"), Some(vec![0xfe, 0x12, 0x3f]));
    }

    #[test]
    fn base64_ids_decode_and_escape_a_marker_byte() {
        assert_eq!(encode_id("AAAA"), Some(vec![0, 0, 0]));
        assert_eq!(encode_id("AQ"), Some(vec![1]));
        assert_eq!(encode_id("_w"), Some(vec![0xfd, 0xff]));
        assert_eq!(encode_id("-A"), Some(vec![0xf8]));
        // Four characters may end in any of them; five never decode.
        assert_eq!(encode_id("abcd"), Some(vec![0x69, 0xb7, 0x1d]));
        assert_eq!(encode_id("abcdA"), Some(b"\xffabcdA".to_vec()));
        // Two characters end in one of four, three in one of sixteen.
        assert_eq!(encode_id("AE"), Some(b"\xffAE".to_vec()));
        assert_eq!(encode_id("AAE"), Some(vec![0, 1]));
        assert_eq!(encode_id("AAA0"), Some(vec![0, 0, 52]));
    }

    #[test]
    fn other_ids_are_utf8_after_a_marker() {
        assert_eq!(encode_id("a"), Some(vec![0xff, b'a']));
        assert_eq!(encode_id("AB"), Some(vec![0xff, b'A', b'B']));
        assert_eq!(
            encode_id("doc 1"),
            Some(vec![0xff, b'd', b'o', b'c', b' ', b'1'])
        );
        assert_eq!(encode_id(""), None);
    }

    #[test]
    fn every_kind_reads_back_as_written() {
        for id in [
            "1", "42", "007", "AAAA", "AQ", "_w", "abcd", "x", "doc 1", "é",
        ] {
            assert_eq!(round_trip(id), format!("\"{id}\""), "{id}");
        }
    }
}
