//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;

const RECORDED: &str = include_str!("../../../fixtures/date-format/java-3.10.0.txt");

fn text(pattern: &DatePattern, key: i64, zone_ms: i64) -> io::Result<String> {
    let mut out = Vec::new();
    pattern.write(&mut out, key, zone_ms)?;
    Ok(String::from_utf8(out).expect("utf-8"))
}

#[test]
fn every_recorded_print_is_what_java_wrote() {
    let (mut checked, mut errors, mut skipped) = (0, 0, Vec::new());
    for line in RECORDED.lines() {
        let mut fields = line.splitn(4, '|');
        let (format, offset, millis, expected) = (
            fields.next().expect("format"),
            fields.next().expect("offset"),
            fields.next().expect("millis"),
            fields.next().expect("text"),
        );
        let Some(pattern) = DatePattern::parse(format) else {
            if !skipped.contains(&format) {
                skipped.push(format);
            }
            continue;
        };
        let zone_ms = offset.parse::<i64>().expect("offset") * 1_000;
        let got = text(&pattern, millis.parse().expect("millis"), zone_ms);
        if expected.starts_with("ERR") {
            assert!(got.is_err(), "{line}");
            errors += 1;
        } else {
            assert_eq!(got.as_deref().ok(), Some(expected), "{line}");
            checked += 1;
        }
    }
    // Only epoch_second and the default format, which has its own printer.
    assert_eq!(skipped, ["strict_date_optional_time", "epoch_second"]);
    assert!(checked > 200 && errors == 1, "{checked} {errors}");
}

#[test]
fn a_letter_or_name_this_node_does_not_print_is_not_read() {
    for format in [
        "yyyy-MM-dd'T'HH:mm:ssZ",
        "MMM",
        "EEE",
        "yyy",
        "'open",
        "",
        "week_date",
        "yyyy[MM]",
    ] {
        assert_eq!(DatePattern::parse(format), None, "{format}");
    }
}

#[test]
fn a_quote_pair_is_one_quote() {
    let p = DatePattern::parse("yyyy''MM").expect("reads");
    assert_eq!(text(&p, 0, 0).expect("writes"), "1970'01");
}
