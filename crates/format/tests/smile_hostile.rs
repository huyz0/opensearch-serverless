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
