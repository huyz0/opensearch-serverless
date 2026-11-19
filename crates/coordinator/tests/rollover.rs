//! docs/specs/coordinator-rollover: a rollover is decided from the body and
//! what the target says.

// ARITH: test values are small constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_coordinator::indices::rollover::{
    AliasFacts, Ask, Condition, Target, backing_index_name, check_target, evaluate, next_name,
    parse_body,
};
use oss_coordinator::route::resolve::Refusal;

fn refused(body: &str) -> (u16, &'static str, String) {
    let r = parse_body(Some(body.as_bytes())).expect_err("refused");
    (r.status, r.kind, r.reason)
}

fn ask(body: &str) -> Ask {
    parse_body(Some(body.as_bytes())).expect("accepted")
}

#[test]
fn the_next_name_increments_a_trailing_number() {
    let next = |n: &str| next_name(n).unwrap();
    assert_eq!(next("logs-000001"), Some("logs-000002".to_owned()));
    assert_eq!(next("logs-1"), Some("logs-000002".to_owned()));
    assert_eq!(next("a-b-000099"), Some("a-b-000100".to_owned()));
    assert_eq!(next("logs-999999"), Some("logs-1000000".to_owned()));
    assert_eq!(next("logs"), None);
    assert_eq!(next("logs-"), None);
    assert_eq!(next("logs-v2"), None);
    assert_eq!(next("logs-000a"), None);
    assert_eq!(next("-1"), Some("-000002".to_owned()));
    let exhausted: Refusal = next_name("x-9223372036854775807").unwrap_err();
    assert_eq!(
        (exhausted.status, exhausted.kind, exhausted.reason.as_str()),
        (
            400,
            "illegal_argument_exception",
            "numeric index suffix is exhausted at [9223372036854775807]"
        )
    );
    let too_long = next_name("x-99999999999999999999").unwrap_err();
    assert_eq!(too_long.status, 400);
    assert_eq!(backing_index_name("web", 7), ".ds-web-000007");
    assert_eq!(backing_index_name("web", 1234567), ".ds-web-1234567");
}

#[test]
fn the_body_names_only_supported_object_fields_and_conditions() {
    let none = parse_body(None).unwrap();
    assert!(none.conditions.is_empty() && none.create.is_none() && !none.shapes_index);
    let none = parse_body(Some(b"")).unwrap();
    assert!(none.conditions.is_empty());
    let none = ask("{}");
    assert!(none.conditions.is_empty() && none.create.is_none());

    let field = |k: &str| {
        (
            400,
            "malformed_body",
            format!("rollover field [{k}] must be a supported object-valued field"),
        )
    };
    assert_eq!(refused(r#"{"max_docs": 5}"#), field("max_docs"));
    assert_eq!(refused(r#"{"conditions": 5}"#), field("conditions"));
    assert_eq!(refused(r#"{"settings": null}"#), field("settings"));
    assert_eq!(refused(r#"{"mappings": []}"#), field("mappings"));
    assert_eq!(
        refused(r#"{"conditions": {}, "extra": {}}"#),
        field("extra")
    );
    assert_eq!(
        refused("{"),
        (
            400,
            "malformed_body",
            "the body is not a JSON object".to_owned()
        )
    );

    let size = "store sizes are not reported here, so a size condition cannot be evaluated";
    for k in ["max_size", "max_size_bytes", "max_primary_shard_size"] {
        assert_eq!(
            refused(&format!(r#"{{"conditions": {{"{k}": "5gb"}}}}"#)),
            (501, "unsupported_condition", size.to_owned()),
            "{k}"
        );
    }
    assert_eq!(
        refused(r#"{"conditions": {"max_docs": 1, "min_age": "1d"}}"#),
        (
            400,
            "unsupported_condition",
            "unknown condition [min_age]".to_owned()
        )
    );
    assert_eq!(
        refused(r#"{"conditions": {"max_age": "soon"}}"#).1,
        "malformed_body"
    );
    assert_eq!(
        refused(r#"{"conditions": {"max_docs": "many"}}"#).1,
        "malformed_body"
    );
    assert_eq!(
        refused(r#"{"conditions": {"max_docs": 1.5}}"#).1,
        "malformed_body"
    );

    let a = ask(r#"{"conditions": {"max_age": "7d", "max_docs": 1000}}"#);
    assert_eq!(a.conditions.len(), 2);
    match (&a.conditions[0], &a.conditions[1]) {
        (Condition::MaxAge(age), Condition::MaxDocs(docs)) => {
            assert_eq!((age.human(), *docs), ("7d".to_owned(), 1000));
        }
        other => panic!("{other:?}"),
    }
    // A number may be written as a string, as core's parser takes it.
    let a = ask(r#"{"conditions": {"max_docs": "42"}}"#);
    assert!(matches!(a.conditions[0], Condition::MaxDocs(42)));
}

#[test]
fn the_body_shapes_the_new_index_as_a_create_request() {
    let a = ask(r#"{"conditions": {"max_docs": 1}}"#);
    assert!(a.create.is_none() && !a.shapes_index && a.aliases_refusal().is_none());

    let a = ask(r#"{"mappings": {"properties": {"a": {"type": "keyword"}}}}"#);
    assert!(a.shapes_index);
    assert_eq!(
        a.create.as_ref().unwrap().mapping.as_deref(),
        Some(r#"{"properties":{"a":{"type":"keyword"}}}"#)
    );
    // An empty mapping is no mapping, and shapes nothing.
    let a = ask(r#"{"mappings": {}}"#);
    assert!(!a.shapes_index);
    assert!(a.create.as_ref().is_none_or(|c| c.mapping.is_none()));

    for body in [
        r#"{"settings": {"refresh_interval": "1s"}}"#,
        r#"{"settings": {"number_of_shards": 2}}"#,
        r#"{"settings": {"number_of_replicas": 0}}"#,
    ] {
        assert!(ask(body).shapes_index, "{body}");
    }
    assert!(!ask(r#"{"settings": {}}"#).shapes_index);
    let a = ask(r#"{"settings": {"number_of_shards": 3, "refresh_interval": "1s"}}"#);
    let create = a.create.unwrap();
    assert_eq!(create.shards, 3);
    assert_eq!(create.settings.len(), 1);

    // Create refusals pass through.
    assert_eq!(
        refused(r#"{"settings": {"number_of_replicas": 1}}"#).1,
        "unsupported_setting"
    );

    let a = ask(r#"{"aliases": {"x": {}}}"#);
    let r = a.aliases_refusal().unwrap();
    assert_eq!(
        (r.status, r.kind, r.reason.as_str()),
        (
            501,
            "unsupported_setting",
            "'aliases' in a rollover body is not supported here; add them to the new index with POST /_aliases"
        )
    );
    assert!(ask(r#"{"aliases": {}}"#).aliases_refusal().is_none());
    assert!(!ask(r#"{"aliases": {"x": {}}}"#).shapes_index);
}

fn alias(data_stream: bool, mutation_pending: bool, write: Option<&str>) -> Target {
    Target::Alias(AliasFacts {
        data_stream,
        mutation_pending,
        write_index: write.map(str::to_owned),
    })
}

fn target_refusal(
    t: &Target,
    new_index: Option<&str>,
    shapes: bool,
) -> (u16, &'static str, String) {
    let r = check_target("t", t, new_index, shapes).expect_err("refused");
    (r.status, r.kind, r.reason)
}

#[test]
fn the_target_must_be_a_rollable_alias() {
    assert_eq!(
        target_refusal(&Target::Index, None, false),
        (
            400,
            "illegal_argument_exception",
            "rollover target [t] is an index; roll over an alias or a data stream".to_owned()
        )
    );
    assert_eq!(
        target_refusal(&Target::Missing, None, false),
        (
            404,
            "index_not_found_exception",
            "no such alias or data stream: t".to_owned()
        )
    );
    assert_eq!(
        target_refusal(&alias(false, true, Some("a")), None, false),
        (
            409,
            "version_conflict_engine_exception",
            "alias is being changed: t".to_owned()
        )
    );
    assert_eq!(
        target_refusal(&alias(true, false, Some("a")), Some("n"), false),
        (
            400,
            "illegal_argument_exception",
            "new index name may not be specified when rolling over a data stream".to_owned()
        )
    );
    let shaped = (
        400,
        "illegal_argument_exception",
        "aliases, mappings, and index settings may not be specified when rolling over a data stream"
            .to_owned(),
    );
    assert_eq!(
        target_refusal(&alias(true, false, Some("a")), None, true),
        shaped
    );
    // The new name is named first when both are given.
    assert_eq!(
        target_refusal(&alias(true, false, Some("a")), Some("n"), true).2,
        "new index name may not be specified when rolling over a data stream"
    );
    assert_eq!(
        target_refusal(&alias(false, false, None), None, false),
        (
            400,
            "illegal_argument_exception",
            "rollover target [t] does not point to a write index".to_owned()
        )
    );
    // The order: pending before data stream before no write index.
    assert_eq!(
        target_refusal(&alias(true, true, None), Some("n"), true).0,
        409
    );
    assert!(
        target_refusal(&alias(true, false, None), Some("n"), true)
            .2
            .starts_with("new index")
    );
    // A plain alias may take a new name and a shape.
    assert!(check_target("t", &alias(false, false, Some("a")), Some("n"), true).is_ok());
    assert!(check_target("t", &alias(true, false, Some("a")), None, false).is_ok());
}

#[test]
fn conditions_are_evaluated_against_what_is_recorded() {
    let age = |text: &str| {
        Condition::MaxAge(oss_coordinator::indices::time_value::parse(text, "max_age").unwrap())
    };
    let day = 86_400_000_i64;
    let never = &mut || -> Result<u64, Refusal> { panic!("no document count was needed") };

    let e = evaluate(&[], "idx", 1, 100, never).unwrap();
    assert!(e.met.is_empty() && e.should_roll);

    let e = evaluate(&[age("1d")], "idx", 1_000, 1_000 + day, never).unwrap();
    assert_eq!(e.met, vec![("[max_age: 1d]".to_owned(), true)]);
    assert!(e.should_roll);
    let e = evaluate(&[age("1d")], "idx", 1_000, 1_000 + day - 1, never).unwrap();
    assert_eq!(e.met, vec![("[max_age: 1d]".to_owned(), false)]);
    assert!(!e.should_roll);

    let count = |n: u64| move || -> Result<u64, Refusal> { Ok(n) };
    let e = evaluate(&[Condition::MaxDocs(10)], "idx", 1, 2, &mut count(10)).unwrap();
    assert_eq!(e.met, vec![("[max_docs: 10]".to_owned(), true)]);
    let e = evaluate(&[Condition::MaxDocs(10)], "idx", 1, 2, &mut count(9)).unwrap();
    assert_eq!(e.met, vec![("[max_docs: 10]".to_owned(), false)]);
    assert!(!e.should_roll);

    // Any met condition rolls; all are reported, in order.
    let e = evaluate(
        &[age("1d"), Condition::MaxDocs(10)],
        "idx",
        1_000,
        1_001,
        &mut count(11),
    )
    .unwrap();
    assert_eq!(
        e.met,
        vec![
            ("[max_age: 1d]".to_owned(), false),
            ("[max_docs: 10]".to_owned(), true)
        ]
    );
    assert!(e.should_roll);

    // An index with no recorded creation time cannot be aged.
    let r = evaluate(&[age("1d")], "idx", 0, 10, never).unwrap_err();
    assert_eq!(
        (r.status, r.kind, r.reason.as_str()),
        (
            501,
            "unsupported_condition",
            "max_age cannot be evaluated: [idx] predates creation times being recorded"
        )
    );
    // A count that fails is that refusal.
    let failing = &mut || -> Result<u64, Refusal> {
        Err(Refusal::new(
            503,
            "count_incomplete",
            "only 1 of 2 shards answered",
        ))
    };
    let r = evaluate(&[Condition::MaxDocs(1)], "idx", 1, 2, failing).unwrap_err();
    assert_eq!((r.status, r.kind), (503, "count_incomplete"));
}
