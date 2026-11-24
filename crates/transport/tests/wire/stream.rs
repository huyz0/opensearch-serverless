//! AC-1 and AC-2: the stream primitives.

use oss_transport::WireError;
use oss_transport::stream::{Input, Output, TaskId};
use oss_transport::version::WireVersion;

use crate::{bytes, hex, rows, unhex};

/// Writes with `write` and returns the bytes.
fn written(write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>) -> String {
    let mut buf = Vec::new();
    write(&mut Output::new(&mut buf, 1 << 20)).unwrap();
    hex(&buf)
}

/// Reads every recorded value back from its bytes, then refuses each cut of
/// them short.
fn reads_back<T: PartialEq + std::fmt::Debug>(
    encoded: &[u8],
    expected: &T,
    read: impl Fn(&mut Input<'_>) -> Result<T, WireError>,
) {
    let mut input = Input::new(encoded);
    assert_eq!(&read(&mut input).unwrap(), expected, "{}", hex(encoded));
    input.finish("a value").unwrap();
    for cut in 0..encoded.len() {
        let mut input = Input::new(&encoded[..cut]);
        assert!(
            matches!(read(&mut input), Err(WireError::Truncated { .. })),
            "{} cut at {cut}",
            hex(encoded)
        );
    }
}

fn csv(text: &str) -> Vec<String> {
    let text = unhex(text);
    if text.is_empty() {
        Vec::new()
    } else {
        text.split(',').map(str::to_owned).collect()
    }
}

#[test]
fn every_recorded_primitive_is_the_javas_both_ways() {
    let rows = rows("stream.txt");
    assert!(rows.len() > 380);
    for row in &rows {
        let (kind, value, expected) = (row[0].as_str(), row[1].as_str(), row[2].as_str());
        let encoded = bytes(expected);
        match kind {
            "vint" => {
                let v: i32 = value.parse().unwrap();
                assert_eq!(written(|o| o.vint(v)), expected, "vint {v}");
                reads_back(&encoded, &v, |i| i.vint("v"));
            }
            "int" => {
                let v: i32 = value.parse().unwrap();
                assert_eq!(written(|o| o.int(v)), expected);
                reads_back(&encoded, &v, |i| i.int("v"));
            }
            "vlong" => {
                let v: i64 = value.parse().unwrap();
                assert_eq!(written(|o| o.vlong(v)), expected, "vlong {v}");
                reads_back(&encoded, &v, |i| i.vlong("v"));
            }
            "zlong" => {
                let v: i64 = value.parse().unwrap();
                assert_eq!(written(|o| o.zlong(v)), expected, "zlong {v}");
                reads_back(&encoded, &v, |i| i.zlong("v"));
            }
            "long" => {
                let v: i64 = value.parse().unwrap();
                assert_eq!(written(|o| o.long(v)), expected);
                reads_back(&encoded, &v, |i| i.long("v"));
            }
            "bool" => {
                let v = value == "true";
                assert_eq!(written(|o| o.bool(v)), expected);
                reads_back(&encoded, &v, |i| i.bool("v"));
            }
            "version" => {
                let v = WireVersion::from_wire_id(value.parse().unwrap());
                assert_eq!(written(|o| v.write(o)), expected);
                reads_back(&encoded, &v, WireVersion::read);
            }
            "string" => {
                let v = unhex(value);
                assert_eq!(written(|o| o.string(&v)), expected, "string {v:?}");
                reads_back(&encoded, &v, |i| i.string("v"));
            }
            "optional_string" => {
                let v = (value != "-").then(|| unhex(value));
                assert_eq!(written(|o| o.optional_string(v.as_deref())), expected);
                reads_back(&encoded, &v, |i| i.optional_string("v"));
            }
            "string_array" => {
                let v = csv(value);
                let refs: Vec<&str> = v.iter().map(String::as_str).collect();
                assert_eq!(written(|o| o.string_array(&refs)), expected);
                reads_back(&encoded, &v, |i| i.string_array("v"));
            }
            "string_map" => {
                let v: Vec<(String, String)> = csv(value)
                    .iter()
                    .map(|kv| {
                        let (k, v) = kv.split_once('=').unwrap();
                        (k.to_owned(), v.to_owned())
                    })
                    .collect();
                assert_eq!(written(|o| o.string_map(&v)), expected);
                reads_back(&encoded, &v, |i| i.string_map("v"));
            }
            "task_id" => {
                let v = match value.split_once(':') {
                    None => TaskId::EMPTY,
                    Some((node, id)) => TaskId {
                        task: Some((node.to_owned(), id.parse().unwrap())),
                    },
                };
                assert_eq!(written(|o| v.write(o)), expected);
                reads_back(&encoded, &v, TaskId::read);
            }
            other => panic!("unknown row kind {other}"),
        }
    }
}

#[test]
fn malformed_values_are_refused() {
    let malformed = |bytes: &[u8], read: &dyn Fn(&mut Input<'_>) -> Result<(), WireError>| {
        assert!(
            matches!(
                read(&mut Input::new(bytes)),
                Err(WireError::Malformed { .. })
            ),
            "{}",
            hex(bytes)
        );
    };
    malformed(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01], &|i| {
        i.vint("v").map(drop)
    });
    malformed(&[0xff; 10], &|i| i.vlong("v").map(drop));
    malformed(
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
        &|i| i.vlong("v").map(drop),
    );
    let mut top = Input::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
    assert_eq!(top.vlong("v").unwrap(), i64::MIN);
    malformed(&[0xff; 11], &|i| i.zlong("v").map(drop));
    malformed(&[2], &|i| i.bool("v").map(drop));
    // A negative count.
    malformed(&[0xff, 0xff, 0xff, 0xff, 0x0f], &|i| {
        i.string("v").map(drop)
    });
    // A byte no char starts with (0xf0), and a lone high surrogate.
    malformed(&[1, 0xf0], &|i| i.string("v").map(drop));
    malformed(&[1, 0x80], &|i| i.string("v").map(drop));
    malformed(&[1, 0xed, 0xa0, 0x80], &|i| i.string("v").map(drop));
    // Bytes left over.
    malformed(&[0, 0], &|i| {
        i.bool("v")?;
        i.finish("v")
    });
    // A negative vlong is never written.
    let mut buf = Vec::new();
    assert!(matches!(
        Output::new(&mut buf, 16).vlong(-1),
        Err(WireError::Malformed { .. })
    ));
}

#[test]
fn a_count_the_bytes_cannot_hold_is_refused_before_anything_is_sized() {
    // A string of a billion chars in five bytes.
    let mut input = Input::new(&[0x80, 0x94, 0xeb, 0xdc, 0x03]);
    assert!(matches!(
        input.string("v"),
        Err(WireError::Truncated { .. })
    ));
    // A map of 3 entries in 3 bytes: each needs at least two.
    let mut input = Input::new(&[3, 0, 0]);
    assert!(matches!(
        input.string_map("v"),
        Err(WireError::Truncated { .. })
    ));
}

#[test]
fn a_write_past_the_limit_is_refused_and_writes_nothing() {
    let mut buf = vec![9u8];
    let mut out = Output::new(&mut buf, 4);
    out.byte(1).unwrap();
    assert!(matches!(
        out.int(7),
        Err(WireError::TooLarge {
            limit: 4,
            actual: 6,
            ..
        })
    ));
    assert_eq!(out.len(), 2);
    assert!(!out.is_empty());
    assert_eq!(out.as_slice(), &[9, 1]);
    out.bytes(&[2, 3]).unwrap();
    assert!(matches!(out.string("x"), Err(WireError::TooLarge { .. })));
    assert_eq!(buf, vec![9, 1, 2, 3]);
    let mut empty = Vec::new();
    assert!(Output::new(&mut empty, 4).is_empty());
    // The last two-byte char and the first three-byte one.
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 16);
    out.string("\u{7ff}\u{800}").unwrap();
    assert_eq!(buf, [2, 0xdf, 0xbf, 0xe0, 0xa0, 0x80]);
}

#[test]
fn reading_tracks_its_position() {
    let mut input = Input::new(&[1, 2, 3]);
    assert_eq!(input.byte("v").unwrap(), 1);
    assert_eq!((input.position(), input.remaining()), (1, 2));
    assert_eq!(input.rest(), &[2, 3]);
    assert_eq!(
        input.bytes_reference("v").unwrap_err(),
        WireError::Truncated { what: "v" }
    );
}

#[test]
fn versions_mask_and_accept_only_the_recorded_one() {
    assert_eq!(WireVersion::SHELL.wire_id(), 137_317_827);
    assert_eq!(WireVersion::SHELL.id(), 3_100_099);
    assert_eq!(WireVersion::from_wire_id(137_317_827), WireVersion::SHELL);
    assert!(WireVersion::SHELL.accepted());
    assert_eq!(WireVersion::SHELL.check_accepted(), Ok(WireVersion::SHELL));
    assert!(!WireVersion::MIN_COMPAT.accepted());
    assert_eq!(
        WireVersion::from_id(3_030_099).check_accepted(),
        Err(WireError::UnsupportedVersion { id: 3_030_099 })
    );
}

#[test]
fn errors_say_what_failed() {
    let cases = [
        (
            WireError::Truncated { what: "a string" },
            "a string is truncated",
        ),
        (
            WireError::Malformed {
                what: "a vint",
                reason: "too long",
            },
            "a vint is malformed: too long",
        ),
        (
            WireError::TooLarge {
                what: "a frame",
                limit: 4,
                actual: 9,
            },
            "a frame is too large: 9 past the limit of 4",
        ),
        (
            WireError::UnsupportedVersion { id: 7 },
            "a peer at wire version 7 is not supported",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
    }
    // A tenth vlong byte of 0 adds nothing.
    let mut input = Input::new(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
    assert_eq!(input.vlong("v").unwrap(), i64::MAX);
}

/// AC-1 of docs/specs/refresh-large-tail: a string of ASCII is its length
/// and its bytes, and one that is not takes the same path both ways as
/// before.
#[test]
fn ascii_and_mixed_strings_round_trip_on_the_same_bytes() {
    let long = "alpha beta ".repeat(1000);
    for text in [
        "",
        "a",
        "{\"a\":1}",
        long.as_str(),
        "é",
        "ab\u{e9}cd",
        "a\u{20ac}",
        "\u{1f600}x",
    ] {
        let mut buf = Vec::new();
        Output::new(&mut buf, 1 << 20).string(text).unwrap();
        let units = text.encode_utf16().count();
        // The count is of UTF-16 units; ASCII is one byte each.
        if text.is_ascii() {
            let mut count = Vec::new();
            Output::new(&mut count, 8)
                .vint(i32::try_from(units).unwrap())
                .unwrap();
            assert_eq!(&buf[..count.len()], &count[..]);
            assert_eq!(&buf[count.len()..], text.as_bytes(), "{text:?}");
        }
        let mut input = Input::new(&buf);
        assert_eq!(input.string("a string").unwrap(), text);
        input.finish("a string").unwrap();
    }
    // A run of ASCII cut short is refused, not read as far as it goes.
    let mut buf = Vec::new();
    Output::new(&mut buf, 64).string("abcdef").unwrap();
    buf.pop();
    assert!(Input::new(&buf).string("a string").is_err());
}
