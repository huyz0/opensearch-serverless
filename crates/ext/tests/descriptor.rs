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

fn with_name(name: &str) -> String {
    GOOD.replace(r#""name":"tokenize_x""#, &format!(r#""name":"{name}""#))
}

fn with_point_id(id: &str) -> String {
    GOOD.replace(r#""id":"analyzer""#, &format!(r#""id":"{id}""#))
}

// docs/specs/extension-abi AC-2
#[test]
fn an_error_never_carries_a_modules_text_raw() {
    let slash = '\\';
    let hostile = format!("x{slash}nERROR admin: granted{slash}u001b[31m{slash}u202e");
    for text in [
        GOOD.replace(r#""io_bytes""#, &format!(r#""{hostile}":1,"io_bytes""#)),
        GOOD.replace(
            r#""io_bytes""#,
            &format!(r#""{hostile}":1,"{hostile}":2,"io_bytes""#),
        ),
    ] {
        let e = parse(&text).expect_err("refused");
        let shown = e.to_string();
        assert!(
            !shown.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "{shown:?}"
        );
        assert!(
            shown.contains("ERROR admin"),
            "the text is kept, made harmless: {shown:?}"
        );
    }
    let long = "k".repeat(10_000);
    let e = parse(&GOOD.replace(r#""io_bytes""#, &format!(r#""{long}":1,"io_bytes""#)))
        .expect_err("refused");
    assert!(
        e.key().len() <= oss_ext::MAX_ERROR_DETAIL_BYTES,
        "{}",
        e.key().len()
    );
    assert!(e.to_string().len() < 400);
}

// docs/specs/extension-abi AC-2, AC-4
#[test]
fn the_ceiling_is_inclusive() {
    let at = format!(
        "{GOOD}{}",
        " ".repeat(MAX_DESCRIPTOR_BYTES as usize - GOOD.len())
    );
    assert_eq!(at.len(), MAX_DESCRIPTOR_BYTES as usize);
    assert!(parse(&at).is_ok(), "exactly the ceiling");
    assert!(parse(&format!("{at} ")).is_err(), "one byte over");
}

// docs/specs/extension-abi AC-2
#[test]
fn sixty_four_points_are_allowed_and_sixty_five_are_not() {
    let points = |n: usize| {
        let list: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"id":"p{i}","version":"1.0.0"}}"#))
            .collect();
        GOOD.replace(
            r#"[{"id":"analyzer","version":"1.0.0"}]"#,
            &format!("[{}]", list.join(",")),
        )
    };
    assert_eq!(parse(&points(64)).expect("64 allowed").points().len(), 64);
    assert_eq!(key_of(&points(65)), "points");
}

// docs/specs/extension-abi AC-2
#[test]
fn the_boundaries_of_each_number_and_name_are_pinned() {
    let top = GOOD.replace(
        r#""max_concurrent_calls":4"#,
        r#""max_concurrent_calls":1024"#,
    );
    assert_eq!(
        parse(&top).expect("1024 allowed").max_concurrent_calls(),
        1024
    );
    let one = GOOD.replace(r#""max_concurrent_calls":4"#, r#""max_concurrent_calls":1"#);
    assert_eq!(parse(&one).expect("1 allowed").max_concurrent_calls(), 1);
    let sixty_four = format!("a{}", "b".repeat(63));
    assert!(parse(&with_name(&sixty_four)).is_ok(), "64 characters");
    assert!(
        parse(&with_name(&format!("{sixty_four}b"))).is_err(),
        "65 characters"
    );
    assert!(parse(&with_name("a")).is_ok(), "one character");
    // A module name may hold `-`, a point id may not; neither may start with a digit, `_` or `-`.
    assert!(parse(&with_name("a-b")).is_ok());
    assert_eq!(key_of(&with_point_id("a-b")), "points[0].id");
    for bad in ["1a", "_a", "-a", "é", "aé", "A", "a b", "a.b"] {
        assert!(parse(&with_name(bad)).is_err(), "{bad}");
        assert!(parse(&with_point_id(bad)).is_err(), "{bad}");
    }
    assert!(parse(&with_point_id("a_b1")).is_ok());
    assert!(parse(&with_name(&"a".repeat(64))).is_ok());
}

// docs/specs/extension-abi AC-2
#[test]
fn versions_are_exactly_numbers_joined_by_dots() {
    for bad in [
        "1.2.3.4",
        "1..3",
        "+1.0.0",
        "1.2.4294967296",
        "1.2.",
        ".1.2",
        "1.2.-3",
        "1.2.3 ",
    ] {
        let text = GOOD.replace(r#""version":"1.2.3""#, &format!(r#""version":"{bad}""#));
        assert_eq!(key_of(&text), "version", "{bad}");
    }
    for bad in [
        "65536.0", "1.0.0", "1.65536", "01.0", "1.00", "a.b", "1.", ".0",
    ] {
        let text = GOOD.replace(r#""abi":"1.0""#, &format!(r#""abi":"{bad}""#));
        assert_eq!(key_of(&text), "abi", "{bad}");
    }
    let max = GOOD.replace(r#""version":"1.2.3""#, r#""version":"4294967295.0.10""#);
    assert_eq!(parse(&max).expect("parses").version(), (u32::MAX, 0, 10));
    let abi = GOOD.replace(r#""abi":"1.0""#, r#""abi":"65535.65535""#);
    assert_eq!(parse(&abi).expect("parses").abi(), (65535, 65535));
}

// docs/specs/extension-abi AC-2
#[test]
fn a_repeated_key_is_found_at_any_depth_and_after_unescaping() {
    let in_limits = GOOD.replace(r#""points""#, r#""limits":{"fuel":1,"fuel":2},"points""#);
    assert_eq!(key_of(&in_limits), "fuel");
    let in_point = GOOD.replace(
        r#"{"id":"analyzer","version":"1.0.0"}"#,
        r#"{"id":"analyzer","id":"b","version":"1.0.0"}"#,
    );
    assert_eq!(key_of(&in_point), "id");
    let slash = '\\';
    let escaped = GOOD.replace(
        r#""io_bytes""#,
        &format!(r#""n{slash}u0061me":"x","io_bytes""#),
    );
    assert_eq!(
        key_of(&escaped),
        "name",
        "an escaped spelling of a key is the same key"
    );
}

// docs/specs/extension-abi AC-2
#[test]
fn nesting_is_allowed_to_the_limit_and_no_further() {
    let nest = |n: usize| {
        let inner = format!("{}1{}", "[".repeat(n), "]".repeat(n));
        GOOD.replace(r#""points""#, &format!(r#""limits":{inner},"points""#))
    };
    // Both are refused as a limits value, but only the deeper one as JSON: the key says which.
    assert_eq!(
        key_of(&nest(7)),
        "limits",
        "within the depth the document parses"
    );
    assert_eq!(
        key_of(&nest(8)),
        "",
        "past the depth it is not accepted as JSON"
    );
}

// docs/specs/extension-abi AC-2
#[test]
fn text_around_the_value_and_odd_escapes_are_refused() {
    for bad in [
        format!("{}{GOOD}", '\u{feff}'),
        format!("{GOOD}\0"),
        format!("{GOOD}/* c */"),
        format!("{GOOD},"),
        GOOD.replace("tokenize_x", "tok\u{0}x"),
    ] {
        assert!(parse(&bad).is_err(), "{bad:?}");
    }
    let slash = '\\';
    for esc in ["udc00", "ud83dA", "ud83d{slash}u0041", "u00zz", "u12"] {
        let esc = esc.replace("{slash}", &slash.to_string());
        let bad = format!("{slash}{esc}");
        assert!(parse(&GOOD.replace("tokenize_x", &bad)).is_err(), "{bad}");
    }
    let nul = format!("a{slash}u0000b");
    let text = GOOD.replace(r#""io_bytes""#, &format!(r#""{nul}":1,"io_bytes""#));
    assert!(
        parse(&text).is_err(),
        "a NUL in a key is just an unknown key, shown harmlessly"
    );
    assert!(!parse(&text).unwrap_err().to_string().contains('\0'));
}
