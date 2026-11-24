//! The strict descriptor parse (docs/specs/extension-abi/SPEC.md AC-2, AC-4).

use oss_ext::{Descriptor, DescriptorError, MAX_DESCRIPTOR_BYTES};

const GOOD: &str = r#"{"name":"tokenize_x","version":"1.2.3","abi":"1.0","io_bytes":65536,
"max_concurrent_calls":4,"points":[{"id":"analyzer","version":"1.0.0"}]}"#;

fn parse(text: &str) -> Result<Descriptor, DescriptorError> {
    Descriptor::parse(text.as_bytes())
}

fn key_of(text: &str) -> String {
    parse(text).expect_err("refused").key().to_owned()
}

// docs/specs/extension-abi AC-2
#[test]
fn a_well_formed_descriptor_is_read_whole() {
    let d = parse(GOOD).expect("parses");
    assert_eq!(d.name(), "tokenize_x");
    assert_eq!(d.version(), (1, 2, 3));
    assert_eq!(d.abi(), (1, 0));
    assert_eq!(d.io_bytes(), 65_536);
    assert_eq!(d.max_concurrent_calls(), 4);
    assert_eq!(d.points().len(), 1);
    assert_eq!(d.points()[0].id(), "analyzer");
    assert_eq!(d.points()[0].version(), (1, 0, 0));
    assert_eq!(d.limits().fuel, None);
}

// docs/specs/extension-abi AC-2
#[test]
fn optional_limits_may_only_ask_for_less() {
    let with = GOOD.replace(
        r#""points""#,
        r#""limits":{"fuel":1000,"memory_bytes":1048576,"deadline_ms":50},"points""#,
    );
    let d = parse(&with).expect("parses");
    assert_eq!(d.limits().fuel, Some(1000));
    assert_eq!(d.limits().memory_bytes, Some(1_048_576));
    assert_eq!(d.limits().deadline_ms, Some(50));
    let zero = GOOD.replace(r#""points""#, r#""limits":{"fuel":0},"points""#);
    assert_eq!(key_of(&zero), "limits.fuel");
}

// docs/specs/extension-abi AC-2
#[test]
fn an_unknown_duplicate_or_missing_key_is_refused_naming_the_key() {
    assert_eq!(
        key_of(&GOOD.replace(r#""abi""#, r#""abi_":"1.0","abi""#)),
        "abi_"
    );
    assert_eq!(
        key_of(&GOOD.replace(r#""io_bytes""#, r#""name":"b","io_bytes""#)),
        "name"
    );
    assert_eq!(key_of(&GOOD.replace(r#""abi":"1.0","#, "")), "abi");
    assert_eq!(
        key_of(&GOOD.replace(r#"{"id":"analyzer","#, r#"{"id":"analyzer","x":1,"#)),
        "points[0].x"
    );
    assert_eq!(
        key_of(&GOOD.replace(
            r#"{"id":"analyzer","version":"1.0.0"}"#,
            r#"{"id":"analyzer"}"#
        )),
        "points[0].version"
    );
}

// docs/specs/extension-abi AC-2
#[test]
fn a_wrong_type_or_value_is_refused_naming_the_key() {
    for (from, to, key) in [
        (r#""io_bytes":65536"#, r#""io_bytes":"big""#, "io_bytes"),
        (r#""io_bytes":65536"#, r#""io_bytes":-1"#, "io_bytes"),
        (r#""io_bytes":65536"#, r#""io_bytes":1.5"#, "io_bytes"),
        (r#""io_bytes":65536"#, r#""io_bytes":0"#, "io_bytes"),
        (
            r#""max_concurrent_calls":4"#,
            r#""max_concurrent_calls":0"#,
            "max_concurrent_calls",
        ),
        (r#""name":"tokenize_x""#, r#""name":"Bad Name""#, "name"),
        (r#""name":"tokenize_x""#, r#""name":"""#, "name"),
        (r#""version":"1.2.3""#, r#""version":"1.2""#, "version"),
        (r#""version":"1.2.3""#, r#""version":"1.2.x""#, "version"),
        (r#""version":"1.2.3""#, r#""version":"01.2.3""#, "version"),
        (r#""abi":"1.0""#, r#""abi":"1""#, "abi"),
        (r#""id":"analyzer""#, r#""id":"Analyzer""#, "points[0].id"),
        (
            r#""points":[{"id":"analyzer","version":"1.0.0"}]"#,
            r#""points":[]"#,
            "points",
        ),
        (
            r#""points":[{"id":"analyzer","version":"1.0.0"}]"#,
            r#""points":{}"#,
            "points",
        ),
    ] {
        assert_eq!(key_of(&GOOD.replace(from, to)), key, "{to}");
    }
    let name_65 = format!(r#""name":"{}""#, "a".repeat(65));
    assert_eq!(
        key_of(&GOOD.replace(r#""name":"tokenize_x""#, &name_65)),
        "name"
    );
}

// docs/specs/extension-abi AC-2
#[test]
fn a_duplicate_point_is_refused() {
    let two = GOOD.replace(
        r#"{"id":"analyzer","version":"1.0.0"}"#,
        r#"{"id":"analyzer","version":"1.0.0"},{"id":"analyzer","version":"2.0.0"}"#,
    );
    assert_eq!(key_of(&two), "points[1].id");
}

// docs/specs/extension-abi AC-2, AC-4
#[test]
fn a_descriptor_over_the_ceiling_or_that_is_not_json_is_refused() {
    let mut big = GOOD.to_owned();
    big.push_str(&" ".repeat(MAX_DESCRIPTOR_BYTES as usize));
    assert_eq!(key_of(&big), "");
    assert!(
        parse(&format!("{GOOD} ")).is_ok(),
        "trailing space is fine under the ceiling"
    );
    for bad in [
        "",
        "[]",
        "null",
        "{",
        r#"{"name":"#,
        "{}x",
        &format!("{GOOD}{GOOD}"),
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
    assert!(Descriptor::parse(&[0xff, 0xfe]).is_err(), "not UTF-8");
}

// docs/specs/extension-abi AC-2
#[test]
fn nesting_is_bounded_so_a_hostile_descriptor_cannot_exhaust_the_stack() {
    let deep = format!("{}1{}", "[".repeat(10_000), "]".repeat(10_000));
    let text = GOOD.replace(r#""points""#, &format!(r#""limits":{deep},"points""#));
    assert!(parse(&text).is_err());
}

// docs/specs/extension-abi AC-2
#[test]
fn a_string_escape_is_read_as_json_reads_it() {
    let slash = '\\';
    let escaped = format!("tokenize{slash}u005fx");
    let d = parse(&GOOD.replace("tokenize_x", &escaped)).expect("parses");
    assert_eq!(d.name(), "tokenize_x");
    let pair = format!("{slash}ud83d{slash}ude00");
    let with_pair = GOOD.replace(
        r#""io_bytes""#,
        &format!(r#""limits":{{"fuel":1}},"x{pair}":1,"io_bytes""#),
    );
    assert_eq!(
        key_of(&with_pair),
        "x\u{1f600}",
        "a surrogate pair is one character"
    );
    let lone = format!("{slash}ud83d");
    assert!(
        parse(&GOOD.replace("tokenize_x", &lone)).is_err(),
        "a lone surrogate"
    );
    assert!(parse(&GOOD.replace("tokenize_x", r"tok\q")).is_err());
}

// docs/specs/extension-abi AC-2
#[test]
fn numbers_are_whole_non_negative_and_fit_or_the_key_is_refused() {
    for (from, to) in [
        ("65536", "18446744073709551616"),
        ("65536", "1e5"),
        ("65536", "6.5536e4"),
        ("65536", "-0"),
        ("65536", "16777217"),
        ("65536", "007"),
    ] {
        let text = GOOD.replace(
            &format!(r#""io_bytes":{from}"#),
            &format!(r#""io_bytes":{to}"#),
        );
        assert!(parse(&text).is_err(), "{to}");
    }
    let top = GOOD.replace(r#""io_bytes":65536"#, r#""io_bytes":16777216"#);
    assert_eq!(
        parse(&top).expect("the ceiling is allowed").io_bytes(),
        16_777_216
    );
    let one = GOOD.replace(r#""io_bytes":65536"#, r#""io_bytes":1"#);
    assert_eq!(parse(&one).expect("one is allowed").io_bytes(), 1);
    let many = GOOD.replace(
        r#""max_concurrent_calls":4"#,
        r#""max_concurrent_calls":1025"#,
    );
    assert_eq!(key_of(&many), "max_concurrent_calls");
}

// docs/specs/extension-abi AC-2
#[test]
fn characters_of_every_width_are_read_whole() {
    for key in ["é", "€", "😀", "aé€😀b"] {
        let text = GOOD.replace(r#""io_bytes""#, &format!(r#""{key}":1,"io_bytes""#));
        assert_eq!(key_of(&text), key);
    }
    let raw_control = GOOD.replace("tokenize_x", "tok\u{1}x");
    assert!(parse(&raw_control).is_err());
    let at_depth = format!(r#"{{"a":{}}}"#, "[".repeat(9) + &"]".repeat(9));
    assert!(parse(&at_depth).is_err());
}
