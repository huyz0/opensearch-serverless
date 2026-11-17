//! Writing JSON as the shell's `XContentBuilder` does: compact, strings
//! escaped, nothing else.

/// Appends `text` as a JSON string, quotes included.
pub(crate) fn push_string(out: &mut Vec<u8>, text: &str) {
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            c if u32::from(c) < 0x20 => {
                let code = u32::from(c);
                let digit = |shift: u32| {
                    b"0123456789abcdef"[usize::try_from((code >> shift) & 15).unwrap_or(0)]
                };
                out.extend_from_slice(b"\\u00");
                out.push(digit(4));
                out.push(digit(0));
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Appends `value` in decimal.
pub(crate) fn push_u64(out: &mut Vec<u8>, value: u64) {
    let mut buffer = [0u8; 20];
    out.extend_from_slice(crate::location::decimal(value, &mut buffer));
}
