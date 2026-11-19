//! docs/specs/coordinator-rollover: the time values a rollover condition reads.

use oss_coordinator::indices::time_value::parse;

fn human(text: &str) -> String {
    parse(text, "max_age").expect("parses").human()
}

#[test]
fn time_values_parse_and_print_as_core_does() {
    let millis = [
        ("7d", 7 * 86_400_000),
        ("12h", 12 * 3_600_000),
        ("30m", 30 * 60_000),
        ("10s", 10_000),
        ("500ms", 500),
        ("2000micros", 2),
        ("3000000nanos", 3),
        (" 5S ", 5_000),
        ("5H", 5 * 3_600_000),
        ("5D", 5 * 86_400_000),
        ("0", 0),
        ("000", 0),
        ("-1", 0),
        ("-01", 0),
    ];
    for (text, want) in millis {
        assert_eq!(parse(text, "x").unwrap().millis(), want, "{text}");
    }
    assert_eq!(parse("7d", "x").unwrap().nanos(), 7 * 86_400_000_000_000);
    let printed = [
        ("7d", "7d"),
        ("90m", "1.5h"),
        ("75m", "1.2h"),
        ("36h", "1.5d"),
        ("1h", "1h"),
        ("30s", "30s"),
        ("90s", "1.5m"),
        ("100ms", "100ms"),
        ("1500ms", "1.5s"),
        ("1micros", "1micros"),
        ("1500micros", "1.5ms"),
        ("5nanos", "5nanos"),
        ("0s", "0s"),
        ("0", "0s"),
        ("-1", "-1"),
        ("-1d", "-1"),
    ];
    for (text, want) in printed {
        assert_eq!(human(text), want, "{text}");
    }
    // Large and saturating values stay positive.
    assert!(parse("9223372036854775807d", "x").unwrap().nanos() > 0);
}

#[test]
fn time_value_refusals_use_the_javas_words() {
    let unit = |v: &str| {
        format!(
            "failed to parse setting [max_age] with value [{v}] as a time value: unit is missing or unrecognized"
        )
    };
    for v in ["7", "7w", "abc", "", "5M", "1.5"] {
        assert_eq!(parse(v, "max_age").unwrap_err(), unit(v), "{v}");
    }
    assert_eq!(
        parse("-2d", "max_age").unwrap_err(),
        "failed to parse setting [max_age] with value [-2d] as a time value: negative durations are not supported"
    );
    assert_eq!(
        parse("1.5h", "max_age").unwrap_err(),
        "failed to parse [1.5h], fractional time values are not supported"
    );
    assert_eq!(parse("xd", "max_age").unwrap_err(), "failed to parse [xd]");
    assert_eq!(parse("d", "max_age").unwrap_err(), "failed to parse [d]");
}
