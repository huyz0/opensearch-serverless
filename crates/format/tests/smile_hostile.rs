//! docs/specs/stock-snapshot-restore AC-1: a SMILE document that breaks a
//! bound or the format is refused, and nothing is allocated past its limits.

use oss_format::{FormatError, SmileLimits, smile_to_json};

/// A header with the given flags (1 names, 2 values, 4 raw binary).
fn doc(flags: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![b':', b')', b'\n', flags];
    v.extend_from_slice(body);
    v
}

fn run(bytes: &[u8], limits: &SmileLimits) -> Result<String, FormatError> {
    let mut out = Vec::new();
    smile_to_json(bytes, limits, &mut out)?;
    Ok(String::from_utf8(out).expect("JSON is UTF-8"))
}

fn plain(bytes: &[u8]) -> Result<String, FormatError> {
    run(bytes, &SmileLimits::default())
}

#[test]
fn a_document_without_the_header_is_refused() {
    assert!(matches!(
        plain(br#"{"a":1}"#),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(plain(b":)"), Err(FormatError::Truncated { .. })));
    assert!(matches!(
        plain(&doc(0x10, &[0x21])),
        Err(FormatError::Unsupported { .. })
    ));
}

#[test]
fn nesting_past_the_depth_limit_is_refused_at_the_limit() {
    let at = |n: usize| {
        let mut body = vec![0xF8; n];
        body.extend(std::iter::repeat_n(0xF9, n));
        doc(0, &body)
    };
    let limits = SmileLimits {
        depth: 64,
        ..SmileLimits::default()
    };
    assert_eq!(run(&at(64), &limits).expect("at the limit").len(), 128);
    assert!(matches!(
        run(&at(65), &limits),
        Err(FormatError::TooLarge { .. })
    ));
    // A bomb of open arrays with no end: stopped at the depth, not at the end.
    let bomb = doc(0, &vec![0xF8; 1 << 20]);
    assert!(matches!(
        run(&bomb, &limits),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_back_reference_past_its_table_is_refused() {
    // A name reference with sharing on and an empty table, and with it off.
    assert!(matches!(
        plain(&doc(1, &[0xFA, 0x40, 0x21, 0xFB])),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        plain(&doc(0, &[0xFA, 0x40, 0x21, 0xFB])),
        Err(FormatError::Corrupt { .. })
    ));
    // The long form, index 1,023, in an empty table.
    assert!(matches!(
        plain(&doc(1, &[0xFA, 0x33, 0xFF, 0x21, 0xFB])),
        Err(FormatError::Corrupt { .. })
    ));
    // Values: a reference with value sharing off, and past the table.
    assert!(matches!(
        plain(&doc(1, &[0xF8, 0x01, 0xF9])),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        plain(&doc(2, &[0xF8, 0x05, 0xF9])),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        plain(&doc(2, &[0xF8, 0xEF, 0xFF, 0xF9])),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_length_that_runs_past_the_input_is_refused_without_allocating_it() {
    // A raw binary of 2^32 bytes, then nothing; and the same in 7-bit form.
    // 2^32, as a variable-length integer.
    let huge = [0x20, 0x00, 0x00, 0x00, 0x80];
    let mut raw = vec![0xFD];
    raw.extend_from_slice(&huge);
    let wide = SmileLimits {
        string_bytes: usize::MAX,
        ..SmileLimits::default()
    };
    assert!(matches!(
        run(&doc(4, &raw), &wide),
        Err(FormatError::Truncated { .. })
    ));
    let mut seven = vec![0xE8];
    seven.extend_from_slice(&huge);
    assert!(matches!(
        run(&doc(0, &seven), &wide),
        Err(FormatError::Truncated { .. })
    ));
    // And past the string limit by default.
    assert!(matches!(
        plain(&doc(4, &raw)),
        Err(FormatError::TooLarge { .. })
    ));
    // A truncated string.
    assert!(matches!(
        plain(&doc(0, &[0x45, b'a'])),
        Err(FormatError::Truncated { .. })
    ));
}

#[test]
fn the_output_limit_stops_a_document_that_writes_too_much() {
    let tiny = SmileLimits {
        output_bytes: 16,
        ..SmileLimits::default()
    };
    // `["aaaaaaaaaaaaaaaaaaaaaaaa"]` is 28 bytes of JSON.
    let mut body = vec![0xF8, 0x60 | 23];
    body.extend(std::iter::repeat_n(b'a', 56));
    body.push(0xF9);
    // 0x60 | 23 is a short ASCII of 56 bytes.
    assert!(matches!(
        run(&doc(0, &body), &tiny),
        Err(FormatError::TooLarge { .. })
    ));
    // A big base64 expansion counts against it.
    let mut bin = vec![0xFD, 0x80 | 30];
    bin.extend(std::iter::repeat_n(7u8, 30));
    assert!(matches!(
        run(&doc(4, &bin), &tiny),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_string_over_the_limit_is_refused() {
    let small = SmileLimits {
        string_bytes: 10,
        ..SmileLimits::default()
    };
    let mut long = vec![0xE0];
    long.extend(std::iter::repeat_n(b'x', 100));
    long.push(0xFC);
    assert!(matches!(
        run(&doc(0, &long), &small),
        Err(FormatError::Corrupt { .. })
    ));
    // Without its end marker.
    assert!(matches!(
        run(&doc(0, &long[..50]), &SmileLimits::default()),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_big_integer_over_its_limit_is_refused() {
    let small = SmileLimits {
        bigint_bytes: 4,
        ..SmileLimits::default()
    };
    // 0x26, length 5, then nothing: refused for its length before its bytes.
    assert!(matches!(
        run(&doc(0, &[0x26, 0x80 | 5]), &small),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn bytes_the_format_does_not_define_are_refused() {
    for body in [
        &[0x00u8][..],
        &[0x27],
        &[0x2B],
        &[0xFF],
        &[0xF8, 0xFB],
        &[0xFA, 0x21, 0x21, 0xFB],
        &[0xFA, 0xF8],
    ] {
        assert!(plain(&doc(0, body)).is_err(), "{body:02x?}");
    }
    // A string that is not UTF-8, a float that is not finite, bytes after
    // the document.
    assert!(plain(&doc(0, &[0x41, 0xFF, 0xFE])).is_err());
    assert!(plain(&doc(0, &[0x28, 0x7, 0x7c, 0x0, 0x0, 0x0])).is_err());
    assert!(plain(&doc(0, &[0x21, 0x21])).is_err());
}

#[test]
fn an_end_marker_after_the_document_is_accepted() {
    assert_eq!(plain(&doc(0, &[0x21, 0xFF])).expect("decodes"), "null");
    assert_eq!(plain(&doc(0, &[0x21])).expect("decodes"), "null");
}

#[test]
fn text_is_escaped() {
    let body = [0x40 | 5, b'a', b'"', b'\\', b'\n', 0x01, b'z'];
    assert_eq!(
        plain(&doc(0, &body)).expect("decodes"),
        r#""a\"\\\n\u0001z""#
    );
}

/// A SMILE vint: 7 bits a byte, the last byte flagged 0x80 and holding 6.
fn vint(mut v: u64) -> Vec<u8> {
    let mut out = vec![0x80 | (v & 0x3F) as u8];
    v >>= 6;
    while v > 0 {
        out.push((v & 0x7F) as u8);
        v >>= 7;
    }
    out.reverse();
    out
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// Bytes as SMILE 7-bit groups: the bit stream cut in sevens, the last cut
/// right-aligned.
fn seven_bit(bytes: &[u8]) -> Vec<u8> {
    let bits: Vec<u8> = bytes
        .iter()
        .flat_map(|b| (0..8).rev().map(move |i| (b >> i) & 1))
        .collect();
    bits.chunks(7)
        .map(|c| c.iter().fold(0u8, |a, b| (a << 1) | b))
        .collect()
}

fn bigint(len: usize) -> Vec<u8> {
    let mut v = vint(len as u64);
    v.extend(seven_bit(&vec![1u8; len]));
    v
}

#[test]
fn a_big_integer_is_refused_one_byte_past_its_limit() {
    let at = |n: usize| {
        let mut b = vec![0x26];
        b.extend(bigint(n));
        doc(0, &b)
    };
    assert!(plain(&at(32)).is_ok());
    assert!(matches!(plain(&at(33)), Err(FormatError::TooLarge { .. })));
}

#[test]
fn big_integers_are_refused_past_their_total() {
    let limits = SmileLimits {
        bigint_total: 64,
        ..SmileLimits::default()
    };
    let two = |n: usize| {
        let mut b = vec![0xF8, 0x26];
        b.extend(bigint(n));
        b.push(0x26);
        b.extend(bigint(n));
        b.push(0xF9);
        doc(0, &b)
    };
    assert!(run(&two(32), &limits).is_ok());
    assert!(matches!(
        run(&two(33), &limits),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_decimal_scale_is_refused_one_past_its_limit() {
    let at = |scale: i64| {
        let mut b = vec![0x2A];
        b.extend(vint(zigzag(scale)));
        b.extend(bigint(1));
        doc(0, &b)
    };
    assert!(plain(&at(10_000)).is_ok());
    assert!(plain(&at(-10_000)).is_ok());
    assert!(matches!(
        plain(&at(10_001)),
        Err(FormatError::TooLarge { .. } | FormatError::Corrupt { .. })
    ));
    assert!(plain(&at(-10_001)).is_err());
}

#[test]
fn an_int_wider_than_32_bits_is_refused() {
    let mut b = vec![0x24];
    b.extend(vint(zigzag(i64::from(i32::MAX))));
    assert_eq!(plain(&doc(0, &b)).expect("i32::MAX"), "2147483647");
    let mut b = vec![0x24];
    b.extend(vint(zigzag(i64::from(i32::MAX) + 1)));
    assert!(plain(&doc(0, &b)).is_err());
}

#[test]
fn the_output_cap_counts_from_where_this_call_started() {
    let limits = SmileLimits {
        output_bytes: 8,
        ..SmileLimits::default()
    };
    let bytes = doc(0, &[0x43, b'a', b'b', b'c', b'd']); // "abcd" is 6 bytes quoted
    let mut out = vec![b'x'; 1000];
    smile_to_json(&bytes, &limits, &mut out).expect("6 bytes fit in 8 after a long prefix");
    assert_eq!(out.len(), 1006);
    let tight = SmileLimits {
        output_bytes: 5,
        ..SmileLimits::default()
    };
    assert!(matches!(
        smile_to_json(&bytes, &tight, &mut Vec::new()),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_shared_name_table_wraps_at_1024_and_the_documents_still_read() {
    // 1,025 distinct names, then the 1,025th again: the table restarted at
    // 1,024, so the name is the first of the new table (index 0).
    let mut body = vec![0xFA];
    for i in 0..1025u32 {
        let name = format!("k{i:04}");
        body.push(0x80 | (name.len() as u8 - 1));
        body.extend(name.as_bytes());
        body.push(0x21);
    }
    body.push(0x40); // shared name 0 of the restarted table: "k1024"
    body.push(0x21);
    body.push(0xFB);
    let json = plain(&doc(1, &body)).expect("wrapped table");
    assert!(json.ends_with(r#""k1024":null,"k1024":null}"#), "{json}");
}

#[test]
fn control_and_multibyte_characters_escape_as_json() {
    let s = "\u{8}\u{c}\u{7f}é\u{1F600}";
    let mut body = vec![0x80 + (s.len() as u8 - 2)];
    body.extend(s.as_bytes());
    let json = plain(&doc(0, &body)).expect("escapes");
    let back: String = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(back, s);
    assert!(
        json.contains("\\u0008") && json.contains("\\u000c"),
        "{json}"
    );
}

#[test]
fn hostile_binary_lengths_are_refused() {
    // 0xFD raw binary claiming far more than the input holds.
    let mut b = vec![0xFD];
    b.extend(vint(u64::from(u32::MAX)));
    assert!(plain(&doc(4, &b)).is_err());
    // 0xE8 7-bit binary the same.
    let mut b = vec![0xE8];
    b.extend(vint(u64::from(u32::MAX)));
    assert!(plain(&doc(0, &b)).is_err());
}

#[test]
fn whitespace_escapes_are_the_short_forms_and_a_space_stays_a_space() {
    // Tiny ASCII "a b\r\t\n" is six bytes.
    let body = [0x45, b'a', b' ', b'b', b'\r', b'\t', b'\n'];
    assert_eq!(plain(&doc(0, &body)).expect("whitespace"), r#""a b\r\t\n""#);
}

#[test]
fn binary_text_fits_the_output_cap_exactly_and_not_a_byte_less() {
    // Three raw bytes are four base64 characters and two quotes: six bytes.
    let mut body = vec![0xFD];
    body.extend(vint(3));
    body.extend([0, 0, 0]);
    let bytes = doc(4, &body);
    let cap = |n: usize| SmileLimits {
        output_bytes: n,
        ..SmileLimits::default()
    };
    assert_eq!(run(&bytes, &cap(6)).expect("fits exactly"), r#""AAAA""#);
    assert!(matches!(
        run(&bytes, &cap(5)),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn short_unicode_strings_read_at_every_length_from_34_to_65_bytes() {
    // 0xA0 is a 34-byte unicode string, 0xBF a 65-byte one.
    for len in [34usize, 40, 65] {
        let s: String = "é".repeat(len / 2) + &"a".repeat(len % 2);
        assert_eq!(s.len(), len);
        let mut body = vec![0xA0 + (len - 34) as u8];
        body.extend(s.as_bytes());
        assert_eq!(
            plain(&doc(0, &body)).expect("short unicode"),
            format!("\"{s}\""),
            "{len} bytes"
        );
    }
}

#[test]
fn a_long_shared_value_reference_reaches_entries_past_255() {
    // 301 distinct four-character strings fill the value table; the
    // reference 0xED 0x2C is (1 << 8) + 44 = 300, the last one.
    let mut body = vec![0xF8];
    for i in 0..301u32 {
        body.push(0x43);
        body.extend(format!("s{i:03}").as_bytes());
    }
    body.extend([0xED, 0x2C, 0xF9]);
    let json = plain(&doc(2, &body)).expect("long reference");
    assert!(json.ends_with(r#","s300","s300"]"#), "{json}");
}

#[test]
fn a_long_shared_name_reference_reaches_entries_past_255() {
    // 301 distinct four-character names fill the table; 0x31 0x2C is
    // (1 << 8) + 44 = 300, the last one.
    let mut body = vec![0xFA];
    for i in 0..301u32 {
        body.push(0x83);
        body.extend(format!("k{i:03}").as_bytes());
        body.push(0x21);
    }
    body.extend([0x31, 0x2C, 0x21, 0xFB]);
    let json = plain(&doc(1, &body)).expect("long name reference");
    assert!(json.ends_with(r#""k300":null,"k300":null}"#), "{json}");
}

#[test]
fn a_binary_is_refused_one_byte_past_the_string_limit() {
    let limits = SmileLimits {
        string_bytes: 3,
        ..SmileLimits::default()
    };
    let of = |n: usize| {
        let mut body = vec![0xFD];
        body.extend(vint(n as u64));
        body.extend(vec![0u8; n]);
        doc(4, &body)
    };
    assert!(run(&of(3), &limits).is_ok());
    assert!(matches!(
        run(&of(4), &limits),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn the_header_flags_say_which_tables_are_shared() {
    // A name, then a reference to it.
    let names = [0xFA, 0x81, b'a', b'b', 0x21, 0x40, 0x21, 0xFB];
    assert!(plain(&doc(1, &names)).is_ok());
    assert!(matches!(
        plain(&doc(2, &names)),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        plain(&doc(0, &names)),
        Err(FormatError::Corrupt { .. })
    ));
    // A value, then a reference to it.
    let values = [0xF8, 0x41, b'a', b'b', 0x01, 0xF9];
    assert_eq!(plain(&doc(2, &values)).expect("shared"), r#"["ab","ab"]"#);
    assert!(matches!(
        plain(&doc(1, &values)),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        plain(&doc(0, &values)),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_header_version_other_than_zero_is_named() {
    for version in [1u8, 2, 15] {
        match plain(&doc(version << 4, &[0x21])) {
            Err(FormatError::Unsupported { version: v, .. }) => {
                assert_eq!(v, u64::from(version));
            }
            other => panic!("{other:?}"),
        }
    }
}
