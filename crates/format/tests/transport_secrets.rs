//! The transport secret register's value, read and written as the shell's
//! `TransportSecrets` reads and writes it (docs/specs/transport/SPEC.md,
//! AC-13).

// ARITH: offsets here are fixture lengths; an overflow panics the test.
#![allow(clippy::disallowed_methods, clippy::arithmetic_side_effects)]

use std::path::PathBuf;

use oss_format::{
    FormatError, MAX_TRANSPORT_SECRET_BYTES, TRANSPORT_SECRET_KEY, TransportSecret,
    transport_secret_key,
};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/transport-secret")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

fn unhex(text: &str) -> Option<String> {
    match text {
        "~" => None,
        "-" => Some(String::new()),
        _ => {
            let bytes: Vec<u8> = (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect();
            Some(String::from_utf8(bytes).unwrap())
        }
    }
}

/// A recorded `generation current previous rotated_at` line.
fn recorded(words: &[&str]) -> TransportSecret {
    TransportSecret {
        generation: words[0].parse().unwrap(),
        current: unhex(words[1]).unwrap(),
        previous: unhex(words[2]),
        rotated_at_millis: words[3].parse().unwrap(),
    }
}

#[test]
fn every_recorded_value_reads_as_the_java_read_it() {
    let cases = fixture("cases.txt");
    let parsed = fixture("parsed.txt");
    let mut refused = 0;
    for (case, expected) in cases.lines().zip(parsed.lines()) {
        let got = TransportSecret::parse(case.as_bytes());
        if expected == "refused" {
            refused += 1;
            assert!(
                matches!(got, Err(FormatError::Corrupt { .. })),
                "{case}: {got:?}"
            );
        } else {
            let words: Vec<&str> = expected.split(' ').collect();
            assert_eq!(got.unwrap(), recorded(&words), "{case}");
        }
    }
    assert_eq!(cases.lines().count(), parsed.lines().count());
    assert_eq!(refused, 12);
}

#[test]
fn every_recorded_value_is_written_as_the_java_wrote_it() {
    let written = fixture("written.txt");
    assert_eq!(written.lines().count(), 3);
    for line in written.lines() {
        let words: Vec<&str> = line.splitn(5, ' ').collect();
        let value = recorded(&words);
        let mut buf = [0u8; MAX_TRANSPORT_SECRET_BYTES];
        let n = value.write(&mut buf).unwrap();
        assert_eq!(std::str::from_utf8(&buf[..n]).unwrap(), words[4]);
        assert_eq!(TransportSecret::parse(&buf[..n]).unwrap(), value);
    }
}

#[test]
fn a_buffer_too_short_is_refused_with_the_length_needed() {
    let value = TransportSecret {
        generation: 1,
        current: "aa11".into(),
        previous: None,
        rotated_at_millis: 0,
    };
    let mut buf = [0u8; 8];
    let err = value.write(&mut buf).unwrap_err();
    assert!(
        matches!(err, FormatError::TooLarge { actual: 64, .. }),
        "{err:?}"
    );
}

#[test]
fn bytes_that_are_not_utf8_are_refused() {
    assert!(matches!(
        TransportSecret::parse(&[0x61, 0xff]),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn neither_debug_nor_an_error_shows_a_key() {
    let value = TransportSecret::parse(br#"{"current":"sekrit","previous":"olds"}"#).unwrap();
    let shown = format!("{value:?}");
    assert!(
        !shown.contains("sekrit") && !shown.contains("olds"),
        "{shown}"
    );
    assert!(shown.contains("previous: true"), "{shown}");
    let err = TransportSecret::parse(br#"{"current":"sekrit","previous":"  "}"#).unwrap_err();
    assert!(!err.to_string().contains("sekrit"), "{err}");
}

#[test]
fn the_key_is_under_the_cluster_config_container() {
    assert_eq!(TRANSPORT_SECRET_KEY, "cluster/config/transport-secret");
    assert_eq!(
        transport_secret_key("b/").unwrap(),
        "b/cluster/config/transport-secret"
    );
    assert!(transport_secret_key("b").is_err());
}
