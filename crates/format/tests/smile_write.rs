//! docs/specs/stock-snapshot-export AC-1: JSON written as SMILE decodes,
//! with the existing decoder, to the JSON it was given; at each bound.

// ARITH: test arithmetic is small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::{SmileLimits, smile_from_json, smile_to_json};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshots/stock-smile")
}

fn encode(json: &str) -> Vec<u8> {
    let mut out = Vec::new();
    smile_from_json(json.as_bytes(), &SmileLimits::default(), &mut out).expect("encodes");
    out
}

fn decode(bytes: &[u8]) -> serde_json::Value {
    let mut out = Vec::new();
    smile_to_json(bytes, &SmileLimits::default(), &mut out).expect("decodes");
    serde_json::from_slice(&out).expect("valid JSON")
}

#[test]
fn every_recorded_json_round_trips() {
    let mut seen = 0;
    for entry in std::fs::read_dir(dir()).unwrap().flatten() {
        let name = entry.file_name().into_string().unwrap();
        if !name.ends_with(".json") || name == "binary.json" {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        let want: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut out = Vec::new();
        match smile_from_json(text.as_bytes(), &SmileLimits::default(), &mut out) {
            Ok(()) => {
                assert_eq!(decode(&out), want, "{name}");
                // For the Java check beside the fixtures (CheckSmileWrite.java).
                if let Some(to) = std::env::var_os("OSS_WRITE_SMILE") {
                    let stem = name.trim_end_matches(".json");
                    std::fs::create_dir_all(&to).unwrap();
                    std::fs::write(PathBuf::from(&to).join(format!("{stem}.rust.smile")), &out)
                        .unwrap();
                }
            }
            // A number past a long is the one thing a stock writer never writes.
            Err(e) => assert!(name == "ints.json", "{name}: {e}"),
        }
        seen += 1;
    }
    assert!(seen >= 9, "{seen}");
}

#[test]
fn the_bytes_of_a_small_document_are_jacksons() {
    // Header with no shared names or values; short ASCII name; small int.
    assert_eq!(
        encode(r#"{"a":1}"#),
        [0x3A, 0x29, 0x0A, 0x00, 0xFA, 0x80, b'a', 0xC2, 0xFB]
    );
    assert_eq!(
        encode(r#"[true,false,null,"","ab",-1]"#),
        [
            0x3A, 0x29, 0x0A, 0x00, 0xF8, 0x23, 0x22, 0x21, 0x20, 0x41, b'a', b'b', 0xC1, 0xF9
        ]
    );
}

#[test]
fn integers_of_every_width_and_strings_of_every_class_round_trip() {
    for n in [
        0i64,
        15,
        16,
        -16,
        -17,
        1 << 20,
        i64::from(i32::MAX),
        i64::from(i32::MIN),
        i64::from(i32::MAX) + 1,
        i64::MAX,
        i64::MIN,
    ] {
        assert_eq!(decode(&encode(&n.to_string())), serde_json::json!(n), "{n}");
    }
    for len in [1usize, 2, 32, 33, 34, 64, 65, 66, 1000] {
        for text in [
            "a".repeat(len),
            "é".repeat(len),
            format!("x\"\\\n{}", "z".repeat(len)),
        ] {
            let json = serde_json::to_string(&text).unwrap();
            assert_eq!(decode(&encode(&json)), serde_json::json!(text), "{len}");
        }
    }
    for name_len in [1usize, 2, 63, 64, 65, 200] {
        let key = "k".repeat(name_len);
        let json = format!(r#"{{"{key}":1,"{}é":2}}"#, key);
        assert_eq!(
            decode(&encode(&json)),
            serde_json::from_str::<serde_json::Value>(&json).unwrap()
        );
    }
}

#[test]
fn a_double_round_trips_by_value() {
    for text in ["1.5", "-0.25", "1e300", "3.141592653589793"] {
        let want: f64 = text.parse().unwrap();
        assert_eq!(decode(&encode(text)).as_f64(), Some(want), "{text}");
    }
}

#[test]
fn the_bounds_hold_at_their_limit_and_one_past() {
    let lim = SmileLimits::default();
    let nest = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
    let mut out = Vec::new();
    assert!(smile_from_json(nest(32).as_bytes(), &lim, &mut out).is_ok());
    assert!(smile_from_json(nest(lim.depth).as_bytes(), &lim, &mut Vec::new()).is_ok());
    assert!(smile_from_json(nest(lim.depth + 1).as_bytes(), &lim, &mut Vec::new()).is_err());
    assert!(smile_from_json(nest(65).as_bytes(), &lim, &mut Vec::new()).is_err());
    let small = SmileLimits {
        string_bytes: 8,
        output_bytes: 64,
        ..lim
    };
    assert!(smile_from_json(br#""12345678""#, &small, &mut Vec::new()).is_ok());
    assert!(smile_from_json(br#""123456789""#, &small, &mut Vec::new()).is_err());
    assert!(smile_from_json(br#"{"123456789":1}"#, &small, &mut Vec::new()).is_err());
    let limit = SmileLimits {
        output_bytes: encode(r#"{"a":[1,2,3]}"#).len(),
        ..lim
    };
    assert!(smile_from_json(br#"{"a":[1,2,3]}"#, &limit, &mut Vec::new()).is_ok());
    assert!(smile_from_json(br#"{"a":[1,2,3,4]}"#, &limit, &mut Vec::new()).is_err());
}

#[test]
fn not_json_and_a_big_integer_are_refused_and_the_buffer_keeps_its_start() {
    for bad in [
        "",
        "{",
        "[1,",
        r#"{"a"}"#,
        "nope",
        "123456789012345678901234567890",
    ] {
        let mut out = b"keep".to_vec();
        assert!(
            smile_from_json(bad.as_bytes(), &SmileLimits::default(), &mut out).is_err(),
            "{bad}"
        );
        assert_eq!(out, b"keep", "{bad}");
    }
}

#[test]
fn each_string_and_name_class_takes_its_shortest_head() {
    // The long form reads back the same, so only the bytes tell the heads apart.
    let head_of = |json: String, at: usize| encode(&json)[at];
    // A document is the 4-byte header, then the value.
    let value = |text: &str| head_of(format!("\"{text}\""), 4);
    assert_eq!(value(&"a".repeat(32)), 0x40 + 31);
    assert_eq!(value(&"a".repeat(33)), 0x60);
    assert_eq!(value(&"a".repeat(64)), 0x60 + 31);
    assert_eq!(value(&"a".repeat(65)), 0xE0);
    assert_eq!(value("é"), 0x80, "a two-byte string is tiny unicode");
    assert_eq!(value("ééé"), 0x80 + 4, "six bytes");
    assert_eq!(
        value(&format!("é{}", "a".repeat(31))),
        0x80 + 31,
        "33 bytes"
    );
    assert_eq!(value(&format!("é{}", "a".repeat(32))), 0xA0, "34 bytes");
    assert_eq!(
        value(&format!("é{}", "a".repeat(63))),
        0xA0 + 31,
        "65 bytes"
    );
    assert_eq!(value(&format!("é{}", "a".repeat(64))), 0xE4, "66 bytes");
    // A name follows the object's start byte.
    let name = |text: &str| head_of(format!("{{\"{text}\":1}}"), 5);
    assert_eq!(name(&"a".repeat(64)), 0x80 + 63);
    assert_eq!(name(&"a".repeat(65)), 0x34);
    assert_eq!(name("é"), 0xC0, "two bytes is the shortest unicode name");
    assert_eq!(name(&format!("é{}", "a".repeat(55))), 0xC0 + 55, "57 bytes");
    assert_eq!(name(&format!("é{}", "a".repeat(56))), 0x34, "58 bytes");
}

#[test]
fn the_writers_own_depth_bound_holds_below_the_readers() {
    let lim = SmileLimits {
        depth: 2,
        ..SmileLimits::default()
    };
    assert!(smile_from_json(b"[[1]]", &lim, &mut Vec::new()).is_ok());
    let mut out = b"keep".to_vec();
    let e = smile_from_json(b"[[[1]]]", &lim, &mut out).unwrap_err();
    assert!(
        matches!(e, oss_format::FormatError::TooLarge { .. }),
        "{e:?}"
    );
    assert_eq!(out, b"keep");
}

#[test]
fn a_limit_hit_partway_leaves_the_buffer_as_it_was() {
    let lim = SmileLimits {
        output_bytes: 8,
        ..SmileLimits::default()
    };
    let mut out = b"keep".to_vec();
    assert!(smile_from_json(br#"{"abc":[1,2,3,4,5,6]}"#, &lim, &mut out).is_err());
    assert_eq!(out, b"keep");
}
