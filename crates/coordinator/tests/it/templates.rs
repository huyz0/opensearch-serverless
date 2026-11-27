//! docs/specs/template-resolution/SPEC.md: which index template an index
//! inherits from, and what it inherits.

use oss_coordinator::templates::{Inherited, TemplateError, matching_template_names, resolve};

fn pick(
    templates: &[(&str, &str)],
    components: &[(&str, &str)],
    index: &str,
) -> Result<Inherited, TemplateError> {
    resolve(
        templates.iter().map(|(n, s)| (*n, s.as_bytes())),
        |name| {
            components
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| s.as_bytes())
        },
        index,
    )
}

fn ok(templates: &[(&str, &str)], index: &str) -> Inherited {
    pick(templates, &[], index).unwrap()
}

fn text(i: &Inherited) -> (String, String, Option<String>) {
    (i.settings.to_json(), i.mappings.to_json(), i.from.clone())
}

#[test]
fn no_template_means_nothing_inherited() {
    let none = ok(&[], "logs-1");
    assert_eq!(text(&none), ("{}".into(), "{}".into(), None));
    assert!(!none.is_data_stream());
    let unmatched = ok(
        &[(
            "t",
            r#"{"index_patterns":["metrics-*"],"template":{"settings":{"a":1}}}"#,
        )],
        "logs-1",
    );
    assert_eq!(unmatched.from, None);
}

#[test]
fn a_matching_template_gives_its_settings_and_mappings() {
    let got = ok(
        &[(
            "t",
            r#"{"index_patterns":["logs-*"],"template":{"settings":{"index":{"refresh_interval":"5s"}},"mappings":{"properties":{"a":{"type":"keyword"}}}}}"#,
        )],
        "logs-1",
    );
    assert_eq!(
        text(&got),
        (
            r#"{"index":{"refresh_interval":"5s"}}"#.into(),
            r#"{"properties":{"a":{"type":"keyword"}}}"#.into(),
            Some("t".into())
        )
    );
}

#[test]
fn patterns_match_the_whole_name_with_stars_as_the_only_wildcard() {
    for (pattern, index, want) in [
        ("logs-*", "logs-1", true),
        ("logs-*", "logs-", true),
        ("logs-*", "xlogs-1", false),
        ("*", "anything", true),
        ("a*b*c", "aXXbYYc", true),
        ("a*b*c", "aXXbYY", false),
        ("a*b*c", "abc", true),
        ("exact", "exact", true),
        ("exact", "exact1", false),
        ("a.c", "abc", false),
        ("a.c", "a.c", true),
        ("l?gs", "logs", false),
        ("*s", "logs", true),
        ("", "", true),
        ("", "x", false),
        ("日*", "日本", true),
    ] {
        let template = format!(r#"{{"index_patterns":["{pattern}"]}}"#);
        let got = ok(&[("t", &template)], index);
        assert_eq!(got.from.is_some(), want, "{pattern} against {index}");
    }
    // A single string stands for a list of one, and any pattern may match.
    assert!(
        ok(&[("t", r#"{"index_patterns":"a*"}"#)], "ab")
            .from
            .is_some()
    );
    assert!(
        ok(&[("t", r#"{"index_patterns":["x","a*"]}"#)], "ab")
            .from
            .is_some()
    );
    assert!(ok(&[("t", r#"{}"#)], "ab").from.is_none());
    assert!(
        ok(&[("t", r#"{"index_patterns":null}"#)], "ab")
            .from
            .is_none()
    );
}

#[test]
fn the_highest_priority_wins_and_a_tie_goes_to_the_smaller_name() {
    let low = r#"{"index_patterns":["*"],"priority":1,"template":{"settings":{"w":"low"}}}"#;
    let high = r#"{"index_patterns":["*"],"priority":2,"template":{"settings":{"w":"high"}}}"#;
    assert_eq!(
        ok(&[("a", low), ("b", high)], "x").from.as_deref(),
        Some("b")
    );
    assert_eq!(
        ok(&[("b", high), ("a", low)], "x").from.as_deref(),
        Some("b")
    );
    let same = r#"{"index_patterns":["*"],"priority":3}"#;
    for order in [[("a", same), ("b", same)], [("b", same), ("a", same)]] {
        assert_eq!(ok(&order, "x").from.as_deref(), Some("a"));
    }
    // No priority is 0, so a template with priority 1 beats it.
    let none = r#"{"index_patterns":["*"]}"#;
    assert_eq!(
        ok(&[("a", none), ("z", low)], "x").from.as_deref(),
        Some("z")
    );
    // Names compare as Java strings do, in UTF-16 order.
    assert_eq!(
        ok(&[("\u{ff5e}", same), ("\u{1f600}", same)], "x")
            .from
            .as_deref(),
        Some("\u{1f600}")
    );
}

#[test]
fn a_priority_is_read_as_core_reads_a_long() {
    for (given, want) in [
        ("5", 5i64),
        ("\"7\"", 7),
        ("5.9", 5),
        ("1e2", 100),
        ("\"2.5\"", 2),
        ("1E+1", 10),
        ("0.9", 0),
        ("9223372036854775807", i64::MAX),
        ("1e0", 1),
        ("\"+4\"", 4),
        ("\".5\"", 0),
        ("\"5.\"", 5),
        ("null", 0),
        ("\"+4.5\"", 4),
        ("\"-0.5\"", 0),
        ("\"-0\"", 0),
        ("0e100", 0),
        ("\"0.000e50\"", 0),
        ("\"12e-1\"", 1),
    ] {
        let template = format!(r#"{{"index_patterns":["*"],"priority":{given}}}"#);
        // A rival at `want - 1` loses, one at `want + 1` wins: the value is `want`.
        let rival = |p: i64| format!(r#"{{"index_patterns":["*"],"priority":{p}}}"#);
        if want > 0 {
            assert_eq!(
                ok(&[("t", &template), ("r", &rival(want - 1))], "x")
                    .from
                    .as_deref(),
                Some("t"),
                "{given}"
            );
        }
        if want < i64::MAX {
            assert_eq!(
                ok(&[("t", &template), ("r", &rival(want + 1))], "x")
                    .from
                    .as_deref(),
                Some("r"),
                "{given}"
            );
        }
    }
    for bad in [
        "-1",
        "\"-1\"",
        "true",
        "\"x\"",
        "[1]",
        "{}",
        "9223372036854775808",
        "1e30",
        "\"\"",
        "-0.5e1",
    ] {
        let template = format!(r#"{{"index_patterns":["*"],"priority":{bad}}}"#);
        let err = pick(&[("t", &template)], &[], "x").unwrap_err();
        assert_eq!(
            err.to_string(),
            "index template priority must be a nonnegative long",
            "{bad}"
        );
    }
    // A template that does not match is not asked its priority.
    let unmatched = r#"{"index_patterns":["zzz"],"priority":-1}"#;
    assert!(pick(&[("t", unmatched)], &[], "x").is_ok());
}

#[test]
fn components_apply_in_order_and_the_template_on_top() {
    let template = r#"{"index_patterns":["*"],"composed_of":["c1","c2"],"template":{"settings":{"index":{"shards":"3"}},"mappings":{"properties":{"own":{"type":"keyword"}}}}}"#;
    let components = [
        (
            "c1",
            r#"{"template":{"settings":{"index":{"shards":"1","codec":"best"},"x":1},"mappings":{"dynamic":"strict","properties":{"a":{"type":"keyword"},"obj":{"properties":{"p":{"type":"long"}}}}}}}"#,
        ),
        (
            "c2",
            r#"{"template":{"settings":{"index":{"codec":"fast"}},"mappings":{"properties":{"obj":{"properties":{"q":{"type":"text"}}},"a":{"type":"text"}}}}}"#,
        ),
    ];
    let got = pick(&[("t", template)], &components, "x").unwrap();
    assert_eq!(
        got.settings.to_json(),
        r#"{"index":{"shards":"3","codec":"fast"},"x":1}"#
    );
    assert_eq!(
        got.mappings.to_json(),
        r#"{"dynamic":"strict","properties":{"a":{"type":"text"},"obj":{"properties":{"p":{"type":"long"},"q":{"type":"text"}}},"own":{"type":"keyword"}}}"#
    );
}

#[test]
fn a_field_replaces_the_one_before_unless_both_are_objects() {
    let template = r#"{"index_patterns":["*"],"composed_of":["c"],"template":{"mappings":{"properties":{"o":{"type":"object","properties":{"y":{"type":"long"}}},"n":{"type":"nested","properties":{"y":{"type":"long"}}},"k":{"type":"text","fields":{"raw":{"type":"keyword"}}},"m":{"type":"keyword"},"i":{"properties":{"y":{"type":"long"}}},"t":{"type":"long"}}}}}"#;
    let component = r#"{"template":{"mappings":{"properties":{"o":{"properties":{"x":{"type":"long"}}},"n":{"type":"nested","properties":{"x":{"type":"long"}}},"k":{"type":"keyword"},"m":{"type":"object","properties":{"x":{"type":"long"}}},"i":{"properties":{"x":{"type":"long"}}},"t":{"properties":{"x":{"type":"long"}}}}}}}"#;
    let got = pick(&[("t", template)], &[("c", component)], "x").unwrap();
    assert_eq!(
        got.mappings.to_json(),
        concat!(
            r#"{"properties":{"o":{"properties":{"x":{"type":"long"},"y":{"type":"long"}},"type":"object"},"#,
            r#""n":{"type":"nested","properties":{"x":{"type":"long"},"y":{"type":"long"}}},"#,
            r#""k":{"type":"text","fields":{"raw":{"type":"keyword"}}},"#,
            r#""m":{"type":"keyword"},"#,
            r#""i":{"properties":{"x":{"type":"long"},"y":{"type":"long"}}},"#,
            r#""t":{"type":"long"}}}"#
        )
    );
}

#[test]
fn arrays_and_scalars_are_replaced_not_merged() {
    let template = r#"{"index_patterns":["*"],"composed_of":["c"],"template":{"settings":{"a":[3],"b":{"x":1},"c":2}}}"#;
    let component = r#"{"template":{"settings":{"a":[1,2],"b":5,"c":{"y":1}}}}"#;
    let got = pick(&[("t", template)], &[("c", component)], "x").unwrap();
    assert_eq!(got.settings.to_json(), r#"{"a":[3],"b":{"x":1},"c":2}"#);
}

#[test]
fn a_component_that_does_not_exist_is_reported() {
    let template = r#"{"index_patterns":["*"],"composed_of":["c1","gone"]}"#;
    let err = pick(&[("t", template)], &[("c1", "{}")], "x").unwrap_err();
    assert_eq!(
        err.to_string(),
        "index template [t] composes component template [gone], which does not exist"
    );
    // A single name stands for a list of one; components are not read at all
    // when no template matches.
    let single = r#"{"index_patterns":["*"],"composed_of":"gone"}"#;
    assert!(pick(&[("t", single)], &[], "x").is_err());
    let unmatched = r#"{"index_patterns":["q"],"composed_of":["gone"]}"#;
    assert!(pick(&[("t", unmatched)], &[], "x").is_ok());
}

#[test]
fn a_stored_template_that_cannot_be_read_is_reported_matching_or_not() {
    for source in ["not json", "[1]", "", r#"{"index_patterns":["*"]"#] {
        let err = pick(&[("bad", source)], &[], "x").unwrap_err();
        assert_eq!(
            err.to_string(),
            "stored template [bad] could not be read",
            "{source}"
        );
    }
    let err = pick(
        &[("t", r#"{"index_patterns":["*"],"composed_of":["c"]}"#)],
        &[("c", "nope")],
        "x",
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "stored template [c] could not be read");
    // A repeated key is refused as the strict parser refuses it.
    assert!(
        pick(
            &[("t", r#"{"index_patterns":["*"],"index_patterns":["x"]}"#)],
            &[],
            "x"
        )
        .is_err()
    );
}

#[test]
fn a_data_stream_template_says_so_and_names_its_timestamp_field() {
    let plain = ok(
        &[("t", r#"{"index_patterns":["*"],"data_stream":{}}"#)],
        "x",
    );
    assert!(plain.is_data_stream());
    assert_eq!(plain.timestamp_field(), "@timestamp");
    let named = ok(
        &[(
            "t",
            r#"{"index_patterns":["*"],"data_stream":{"timestamp_field":{"name":"ts"}}}"#,
        )],
        "x",
    );
    assert_eq!(named.timestamp_field(), "ts");
    assert_eq!(
        named.data_stream.as_ref().unwrap().to_json(),
        r#"{"timestamp_field":{"name":"ts"}}"#
    );
    let odd = ok(
        &[(
            "t",
            r#"{"index_patterns":["*"],"data_stream":{"timestamp_field":"ts"}}"#,
        )],
        "x",
    );
    assert_eq!(odd.timestamp_field(), "@timestamp");
    let nullname = ok(
        &[(
            "t",
            r#"{"index_patterns":["*"],"data_stream":{"timestamp_field":{"name":null}}}"#,
        )],
        "x",
    );
    assert_eq!(nullname.timestamp_field(), "@timestamp");
    assert!(!ok(&[("t", r#"{"index_patterns":["*"],"data_stream":5}"#)], "x").is_data_stream());
    assert!(!ok(&[("t", r#"{"index_patterns":["*"]}"#)], "x").is_data_stream());
}

#[test]
fn many_indices_each_get_their_winning_template_name() {
    let templates = [
        ("broad", r#"{"index_patterns":["*"],"priority":1}"#),
        ("logs", r#"{"index_patterns":["logs-*"],"priority":5}"#),
        ("logs2", r#"{"index_patterns":["logs-*"],"priority":5}"#),
        (
            "metrics",
            r#"{"index_patterns":["metrics-*"],"priority":0}"#,
        ),
    ];
    let got = matching_template_names(
        templates.iter().map(|(n, s)| (*n, s.as_bytes())),
        &["logs-1", "metrics-1", "other", "logs-1"],
    )
    .unwrap();
    assert_eq!(
        got,
        [
            ("logs-1".to_owned(), "logs".to_owned()),
            ("metrics-1".to_owned(), "broad".to_owned()),
            ("other".to_owned(), "broad".to_owned())
        ]
    );
    assert!(
        matching_template_names(std::iter::empty(), &["x"])
            .unwrap()
            .is_empty()
    );
    let none: [&str; 0] = [];
    assert!(
        matching_template_names(templates.iter().map(|(n, s)| (*n, s.as_bytes())), &none)
            .unwrap()
            .is_empty()
    );
    let unmatched = matching_template_names(
        std::iter::once(("t", &b"{\"index_patterns\":[\"q\"]}"[..])),
        &["x"],
    )
    .unwrap();
    assert!(unmatched.is_empty());
}
