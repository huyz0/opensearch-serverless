//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;
use crate::dsl::parse_chunks;

fn values(body: &str) -> Vec<AfterValue> {
    parse_chunks([body.as_bytes()])
        .expect("parses")
        .search_after
}

fn parse_error(body: &str) -> Failure {
    parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure()
}

fn after(values: &[AfterValue], keys: &[AfterKey<'_>]) -> Result<FieldDoc, ShardError> {
    field_doc(0, values, Some(keys))
}

fn one(value: AfterValue, key: AfterKey<'_>) -> Result<i64, ShardError> {
    Ok(after(&[value], &[key])?.values[0])
}

fn text(s: &str) -> AfterValue {
    AfterValue::Text(s.to_owned())
}

/// The status, the exception, its reason and its cause's reason.
fn failed(r: Result<impl std::fmt::Debug, ShardError>) -> (u16, &'static str, String, String) {
    let e = r.expect_err("fails");
    let cause = e
        .failure
        .cause
        .as_ref()
        .map_or(String::new(), |c| c.reason.clone());
    (e.status, e.failure.kind, e.failure.reason, cause)
}

#[test]
fn values_are_read_as_jackson_types_them() {
    assert_eq!(
        values(r#"{"search_after":[1,-2147483649,3000000000,2.5,1e2,"x",true,false,null]}"#),
        [
            AfterValue::Int(1),
            AfterValue::Long(-2_147_483_649),
            AfterValue::Long(3_000_000_000),
            AfterValue::Double(2.5),
            AfterValue::Double(100.0),
            text("x"),
            AfterValue::Bool(true),
            AfterValue::Bool(false),
            AfterValue::Null,
        ]
    );
    // A whole number past a long is kept as its text, as BIG_INTEGER is.
    assert_eq!(
        values(r#"{"search_after":[99999999999999999999,-0]}"#),
        [text("99999999999999999999"), AfterValue::Int(0)]
    );
}

#[test]
fn a_bad_search_after_fails_as_opensearch_fails() {
    let f = parse_error(r#"{"search_after":[]}"#);
    assert_eq!(
        (f.kind, f.reason.as_str()),
        (
            "illegal_argument_exception",
            "Values must contains at least one value."
        )
    );
    for (body, found) in [
        (r#"{"search_after":[{}]}"#, "START_OBJECT"),
        (r#"{"search_after":[1,[2]]}"#, "START_ARRAY"),
    ] {
        let f = parse_error(body);
        assert_eq!(f.kind, "parsing_exception", "{body}");
        assert_eq!(
            f.reason,
            format!(
                "Expected [VALUE_STRING] or [VALUE_NUMBER] or [VALUE_BOOLEAN] or [VALUE_NULL] \
                 but found [{found}] inside search_after."
            )
        );
    }
    let f = parse_error(r#"{"search_after":1}"#);
    assert_eq!(
        f.reason,
        "Unknown key for a VALUE_NUMBER in [search_after]."
    );
}

#[test]
fn the_page_needs_from_zero_a_sort_and_a_value_a_key() {
    let v = [AfterValue::Int(1)];
    let e = field_doc(1, &v, Some(&[AfterKey::Doc])).expect_err("from");
    assert_eq!(
        (e.status, e.failure.kind, e.failure.reason.as_str()),
        (
            500,
            "search_exception",
            "`from` parameter must be set to 0 when `search_after` is used."
        )
    );
    for keys in [None, Some(&[][..])] {
        assert_eq!(
            failed(field_doc(0, &v, keys)),
            (
                400,
                "illegal_argument_exception",
                "Sort must contain at least one field.".to_owned(),
                String::new()
            )
        );
    }
    assert_eq!(
        failed(after(
            &[AfterValue::Int(1), AfterValue::Int(2)],
            &[AfterKey::Doc]
        ))
        .2,
        "search_after has 2 value(s) but sort has 1."
    );
}

#[test]
fn an_int_key_narrows_a_number_and_parses_a_string() {
    let int = AfterKey::Int("year");
    assert_eq!(one(AfterValue::Int(2001), int).ok(), Some(2001));
    // `intValue` wraps a long and truncates a double, saturating.
    assert_eq!(
        one(AfterValue::Long(3_000_000_000), int).ok(),
        Some(-1_294_967_296)
    );
    assert_eq!(one(AfterValue::Double(2001.7), int).ok(), Some(2001));
    assert_eq!(one(AfterValue::Double(-2001.7), int).ok(), Some(-2001));
    assert_eq!(
        one(AfterValue::Double(1e300), int).ok(),
        Some(i64::from(i32::MAX))
    );
    assert_eq!(one(text("+2001"), int).ok(), Some(2001));
    assert_eq!(one(text("-7"), AfterKey::Doc).ok(), Some(-7));
    for (value, input) in [
        (text("2001.7"), "2001.7"),
        (text(""), ""),
        (text(" 1"), " 1"),
        (text("99999999999999999999"), "99999999999999999999"),
        (AfterValue::Bool(true), "true"),
        (AfterValue::Bool(false), "false"),
    ] {
        assert_eq!(
            failed(one(value, int)),
            (
                400,
                "illegal_argument_exception",
                "Failed to parse search_after value for field [year].".to_owned(),
                format!("For input string: \"{input}\"")
            )
        );
    }
    assert_eq!(
        failed(one(text("x"), AfterKey::Doc)).2,
        "Failed to parse search_after value for field [null]."
    );
}

#[test]
fn a_score_key_reads_a_float_as_its_bits() {
    let bits = |f: f32| Some(i64::from(f.to_bits()));
    assert_eq!(one(AfterValue::Int(1), AfterKey::Score).ok(), bits(1.0));
    assert_eq!(
        one(AfterValue::Long(1 << 40), AfterKey::Score).ok(),
        bits(1_099_511_627_776.0)
    );
    assert_eq!(
        one(AfterValue::Double(0.1), AfterKey::Score).ok(),
        bits(0.1)
    );
    assert_eq!(one(text(" 0.5f "), AfterKey::Score).ok(), bits(0.5));
    assert_eq!(one(text("-2.5D"), AfterKey::Score).ok(), bits(-2.5));
    assert_eq!(one(text("NaN"), AfterKey::Score).ok(), bits(f32::NAN));
    assert_eq!(
        one(text("-Infinity"), AfterKey::Score).ok(),
        bits(f32::NEG_INFINITY)
    );
    let (status, kind, reason, cause) = failed(one(text("x"), AfterKey::Score));
    assert_eq!(
        (status, kind, reason.as_str(), cause.as_str()),
        (
            400,
            "illegal_argument_exception",
            "Failed to parse search_after value for field [null].",
            "For input string: \"x\""
        )
    );
    assert_eq!(failed(one(text("  "), AfterKey::Score)).3, "empty String");
    assert_eq!(
        failed(one(text("inf"), AfterKey::Score)).3,
        "For input string: \"inf\""
    );
    assert_eq!(
        failed(one(AfterValue::Bool(true), AfterKey::Score)).3,
        "For input string: \"true\""
    );
}

#[test]
fn a_long_key_parses_a_long_or_a_double_rounded_down() {
    let long = AfterKey::Long("n");
    assert_eq!(one(AfterValue::Long(1 << 40), long).ok(), Some(1 << 40));
    assert_eq!(one(AfterValue::Int(-3), long).ok(), Some(-3));
    assert_eq!(one(AfterValue::Double(-2.5), long).ok(), Some(-2));
    assert_eq!(one(AfterValue::Double(1e300), long).ok(), Some(i64::MAX));
    assert_eq!(one(text("9223372036854775807"), long).ok(), Some(i64::MAX));
    assert_eq!(one(text("-2.5"), long).ok(), Some(-3));
    assert_eq!(one(text("2.5"), long).ok(), Some(2));
    assert_eq!(one(text("1e30"), long).ok(), Some(i64::MAX));
    assert_eq!(one(text("NaN"), long).ok(), Some(0));
    let (status, _, reason, cause) = failed(one(text("x"), long));
    assert_eq!(
        (status, reason.as_str(), cause.as_str()),
        (
            400,
            "Failed to parse search_after value for field [n].",
            "For input string: \"x\""
        )
    );
    assert_eq!(failed(one(text(""), long)).3, "empty String");
}

#[test]
fn a_date_key_parses_a_date_and_refuses_now() {
    let date = AfterKey::Date("published");
    assert_eq!(one(text("2002-01-01"), date).ok(), Some(1_009_843_200_000));
    assert_eq!(
        one(text("1009843200000"), date).ok(),
        Some(1_009_843_200_000)
    );
    assert_eq!(
        one(AfterValue::Double(1.0e12), date).ok(),
        Some(1_000_000_000_000)
    );
    assert_eq!(one(AfterValue::Long(1 << 41), date).ok(), Some(1 << 41));
    let (status, kind, reason, cause) = failed(one(text("now-1d"), date));
    assert_eq!(
        (status, kind, reason.as_str(), cause.as_str()),
        (
            400,
            "parse_exception",
            "could not read the current timestamp",
            "now() is not allowed in [search_after] key"
        )
    );
    assert!(
        failed(one(text("2002-01-01||+1d"), date))
            .2
            .ends_with("is not supported by this node yet")
    );
    let (status, kind, reason, _) = failed(one(AfterValue::Bool(true), date));
    assert_eq!((status, kind), (400, "parse_exception"));
    assert!(
        reason.starts_with("failed to parse date field [true]"),
        "{reason}"
    );
}

#[test]
fn a_keyword_key_takes_the_value_s_text() {
    let keys = [AfterKey::Term, AfterKey::Int("year")];
    let terms = |v: AfterValue| {
        after(&[v, AfterValue::Int(1)], &keys)
            .expect("builds")
            .terms
    };
    let some = |s: &str| Some(s.as_bytes().to_vec());
    assert_eq!(terms(text("animal")), [some("animal"), None]);
    assert_eq!(terms(AfterValue::Int(5)), [some("5"), None]);
    assert_eq!(
        terms(AfterValue::Long(1 << 40)),
        [some("1099511627776"), None]
    );
    assert_eq!(terms(AfterValue::Double(2.5e10)), [some("2.5E10"), None]);
    assert_eq!(terms(AfterValue::Bool(true)), [some("true"), None]);
    assert_eq!(terms(AfterValue::Bool(false)), [some("false"), None]);
    assert_eq!(terms(AfterValue::Null), [None, None]);
    let doc = after(&[text("a"), AfterValue::Int(7)], &keys).expect("builds");
    assert_eq!((doc.doc, doc.values), (i32::MAX, vec![0, 7]));
    // Without a keyword key there are no term slots.
    let doc = after(&[AfterValue::Int(7)], &[AfterKey::Doc]).expect("builds");
    assert!(doc.terms.is_empty());
}

#[test]
fn a_null_for_a_numeric_key_fails_after_every_value_converts() {
    for (key, class) in [
        (AfterKey::Doc, "Integer.intValue"),
        (AfterKey::Int("i"), "Integer.intValue"),
        (AfterKey::Score, "Float.floatValue"),
        (AfterKey::Long("l"), "Long.longValue"),
        (AfterKey::Date("d"), "Long.longValue"),
    ] {
        assert_eq!(
            failed(one(AfterValue::Null, key)),
            (
                500,
                "null_pointer_exception",
                format!("Cannot invoke \"java.lang.{class}()\" because \"value\" is null"),
                String::new()
            )
        );
    }
    // A later value that fails to parse fails first; the first null names
    // the exception.
    let keys = [AfterKey::Score, AfterKey::Doc, AfterKey::Int("i")];
    let nulls = [AfterValue::Null, AfterValue::Null, text("x")];
    assert_eq!(failed(after(&nulls, &keys)).0, 400);
    let nulls = [AfterValue::Null, AfterValue::Null, AfterValue::Int(1)];
    assert!(failed(after(&nulls, &keys)).2.contains("Float.floatValue"));
}

#[test]
fn a_key_is_read_from_its_sort_field() {
    let not_date = |_: &str| false;
    assert_eq!(AfterKey::of(&SortField::score(), not_date), AfterKey::Score);
    assert_eq!(AfterKey::of(&SortField::doc(), not_date), AfterKey::Doc);
    let int = SortField::numeric("year", SortType::Int, false);
    assert_eq!(AfterKey::of(&int, not_date), AfterKey::Int("year"));
    let long = SortField::numeric("n", SortType::Long, false);
    assert_eq!(AfterKey::of(&long, not_date), AfterKey::Long("n"));
    assert_eq!(AfterKey::of(&long, |f| f == "n"), AfterKey::Date("n"));
    let term = SortField::string("tag", false);
    assert_eq!(AfterKey::of(&term, not_date), AfterKey::Term);
}
