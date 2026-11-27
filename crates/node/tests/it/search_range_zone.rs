//! docs/specs/search-range-format-zone: a range's `format` and `time_zone`.

use crate::support::{Node, request, start};

const PATH: &str = "/compat-term/_search";

fn total(node: &Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

fn range(bounds: &str) -> String {
    format!(r#"{{"size":0,"query":{{"range":{{"published":{{{bounds}}}}}}}}}"#)
}

/// The number after `"total":{"value":` in an answer.
fn counted(text: &str) -> u64 {
    let key = r#""total":{"value":"#;
    let from = text.find(key).unwrap_or_else(|| panic!("{text}")) + key.len();
    text[from..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|d| d.parse().ok())
        .expect("a number")
}

fn count_of(node: &Node, bounds: &str) -> (u16, String) {
    total(node, PATH, &range(bounds))
}

#[test]
fn a_range_reads_its_bounds_in_the_zone_and_format_it_names() {
    let node = start();
    // 2001-03-04T00:00:00Z is 983664000000: the first document's date.
    let at_utc = |bound: &str| {
        let (status, text) = count_of(&node, bound);
        assert_eq!(status, 200, "{bound}: {text}");
        counted(&text)
    };
    let one_hour_early = at_utc(r#""lte":"2001-03-03T23:00:00Z""#);
    let on_the_dot = at_utc(r#""lte":"2001-03-04T00:00:00Z""#);
    assert!(
        on_the_dot > one_hour_early,
        "the first document sits on the dot"
    );
    // The same wall clock in the zone an hour east is an hour earlier in UTC.
    assert_eq!(
        at_utc(r#""lte":"2001-03-04T00:00:00","time_zone":"+01:00""#),
        one_hour_early
    );
    assert_eq!(
        at_utc(r#""lte":"2001-03-04T00:00:00","time_zone":"UTC""#),
        on_the_dot
    );
    // A bound with a zone of its own ignores the range's.
    assert_eq!(
        at_utc(r#""lte":"2001-03-04T00:00:00Z","time_zone":"+05:00""#),
        on_the_dot
    );
    // Rounding runs on the zone's calendar: 02:00+01:00 rounded down to the day is 2001-03-03T23:00Z.
    assert_eq!(
        at_utc(r#""lt":"2001-03-04T02:00:00||/d","time_zone":"+01:00""#),
        at_utc(r#""lt":"2001-03-03T23:00:00Z""#)
    );
    // A region reads the wall clock at the offset in force: Paris is an hour east in March.
    assert_eq!(
        at_utc(r#""lte":"2001-03-04T00:00:00","time_zone":"Europe/Paris""#),
        one_hour_early
    );
    // ... and two hours east in July, so the same wall clock is two hours before UTC.
    assert_eq!(
        at_utc(r#""lte":"2001-07-01T00:00:00","time_zone":"Europe/Paris""#),
        at_utc(r#""lte":"2001-06-30T22:00:00Z""#)
    );
    assert_eq!(
        at_utc(r#""lt":"2001-03-04T02:00:00||/d","time_zone":"Europe/Paris""#),
        at_utc(r#""lt":"2001-03-03T23:00:00Z""#)
    );
    for (bound, same_as) in [
        (
            r#""lte":"983664000000","format":"epoch_millis""#,
            on_the_dot,
        ),
        (
            r#""lte":"2001-03-04T00:00:00Z","format":"strict_date_optional_time""#,
            on_the_dot,
        ),
        (
            r#""lte":"2001-03-04","format":"strict_date_optional_time||epoch_millis""#,
            on_the_dot,
        ),
    ] {
        assert_eq!(at_utc(bound), same_as, "{bound}");
    }
}

#[test]
fn a_range_refuses_a_format_or_zone_it_does_not_read() {
    let node = start();
    for bound in [
        r#""lte":"2001","format":"yyyy""#,
        r#""lte":"2001-03-04","time_zone":"Mars/Base""#,
        r#""lte":"2001-03-04","time_zone":"asia/kolkata""#,
        r#""lte":"983664000000","format":"strict_date_optional_time""#,
        r#""lte":"2001-03-04","format":"epoch_millis""#,
    ] {
        let (status, text) = count_of(&node, bound);
        assert_eq!(status, 400, "{bound}: {text}");
        assert!(
            text.contains("not supported by this node yet"),
            "{bound}: {text}"
        );
    }
}
