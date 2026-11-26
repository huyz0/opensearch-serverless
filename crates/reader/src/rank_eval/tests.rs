use super::*;
use crate::dsl::source::tokenizer;
use crate::dsl::{MAX_RECORDED_BYTES, Recorder};

fn parse(body: &str) -> Result<RankEvalSpec, String> {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokens = tokenizer();
    tokens.feed(body.as_bytes(), &mut rec).expect("feeds");
    tokens.finish(&mut rec).expect("finishes");
    RankEvalSpec::parse(&rec).map_err(|e| e.into_failure().reason)
}

const REQ: &str = r#"{"id":"q","request":{"query":{"match_all":{}}},"ratings":[{"_index":"i","_id":"1","rating":2}]}"#;

fn with(request: &str, metric: &str) -> String {
    // ALLOC: test data.
    format!(r#"{{"requests":[{request}],"metric":{metric}}}"#)
}

#[test]
fn a_spec_names_its_requests_metric_and_templates() {
    // ALLOC: test data.
    let body = format!(
        r#"{{"requests":[{REQ},{{"id":"t","template_id":"x","params":{{"a":1}},"ratings":[]}}],"metric":{{"recall":{{"k":3,"relevant_rating_threshold":2}}}},"templates":[{{"id":"x","template":{{"source":{{"query":"{{{{a}}}}"}},"lang":"mustache"}}}},{{"id":"y","template":"{{}}"}}],"max_concurrent_searches":2}}"#
    );
    let spec = parse(&body).expect("parses");
    assert_eq!(spec.metric, Metric::Recall { threshold: 2, k: 3 });
    assert_eq!(spec.requests.len(), 2);
    assert_eq!(
        spec.requests[0].body.as_deref(),
        Some(r#"{"query":{"match_all":{}}}"#)
    );
    assert_eq!(spec.requests[0].ratings[0].1, 2);
    assert_eq!(spec.requests[1].template_id.as_deref(), Some("x"));
    assert_eq!(
        spec.templates[0],
        // ALLOC: test data.
        (
            "x".to_owned(),
            TemplateText::Inline(r#"{"query":"{{a}}"}"#.to_owned())
        )
    );
    // ALLOC: test data.
    assert_eq!(
        spec.templates[1],
        ("y".to_owned(), TemplateText::Inline("{}".to_owned()))
    );
}

// AC-9
#[test]
fn a_template_may_name_a_stored_script() {
    let body = r#"{"requests":[{"id":"t","template_id":"x","params":{"a":1},"ratings":[]}],"metric":{"precision":{}},"templates":[{"id":"x","template":{"id":"stored"}}]}"#;
    let spec = parse(body).expect("parses");
    assert_eq!(
        spec.templates[0],
        // ALLOC: test data.
        ("x".to_owned(), TemplateText::Stored("stored".to_owned()))
    );
    // ALLOC: test data.
    let both = body.replace(r#"{"id":"stored"}"#, r#"{"id":"stored","source":"{}"}"#);
    let e = parse(&both).expect_err("both");
    assert!(e.contains("only one of [source] and [id]"), "{e}");
}

#[test]
fn the_metrics_take_their_defaults_and_parameters() {
    let m = |metric: &str| parse(&with(REQ, metric)).expect("parses").metric;
    assert_eq!(
        m(r#"{"precision":{}}"#),
        Metric::Precision {
            threshold: 1,
            ignore_unlabeled: false,
            k: 10
        }
    );
    assert_eq!(
        m(r#"{"precision":{"ignore_unlabeled":true,"k":4,"relevant_rating_threshold":0}}"#),
        Metric::Precision {
            threshold: 0,
            ignore_unlabeled: true,
            k: 4
        }
    );
    assert_eq!(
        m(r#"{"mean_reciprocal_rank":{}}"#),
        Metric::ReciprocalRank {
            threshold: 1,
            k: 10
        }
    );
    assert_eq!(
        m(r#"{"recall":{}}"#),
        Metric::Recall {
            threshold: 1,
            k: 10
        }
    );
}

#[test]
fn a_body_the_java_parser_refuses_says_why() {
    let cases: [(String, &str); 24] = [
        (
            // ALLOC: test data.
            String::new(),
            "request body or source parameter is required",
        ),
        ("[]".into(), "Expected START_OBJECT"),
        (r#"{"x":1}"#.into(), "unknown field [x]"),
        (
            r#"{"metric":{"precision":{}}}"#.into(),
            "requests is missing",
        ),
        // ALLOC: test data.
        (format!(r#"{{"requests":[{REQ}]}}"#), "metric is missing"),
        (with("", r#"{"precision":{}}"#), "no search requests"),
        (
            r#"{"requests":1}"#.into(),
            "requests doesn't support values of type: VALUE_NUMBER",
        ),
        (
            with(REQ, r#"{"precision":{"k":0}}"#),
            "failed to build [precision]",
        ),
        (with(REQ, r#"{"precision":{"k":"a"}}"#), "k doesn't support"),
        (
            with(REQ, r#"{"precision":{"nope":1}}"#),
            "unknown field [nope]",
        ),
        (
            with(REQ, r#"{"recall":{"ignore_unlabeled":true}}"#),
            "unknown field [ignore_unlabeled]",
        ),
        (
            with(REQ, r#"{"precision":{"ignore_unlabeled":1}}"#),
            "ignore_unlabeled doesn't support",
        ),
        (with(REQ, r#"{"nope":{}}"#), "unknown field [nope]"),
        (with(REQ, r#"{"dcg":{}}"#), "not supported by this node yet"),
        (with(REQ, "{}"), "expected a metric name"),
        (
            with(REQ, r#"{"precision":{},"recall":{}}"#),
            "takes one name",
        ),
        (with(REQ, "[]"), "Expected START_OBJECT"),
        (
            with(r#"{"ratings":[]}"#, r#"{"precision":{}}"#),
            "id is missing",
        ),
        (
            with(r#"{"id":"q"}"#, r#"{"precision":{}}"#),
            "ratings is missing",
        ),
        (
            with(r#"{"id":"q","ratings":[]}"#, r#"{"precision":{}}"#),
            "Need to set at least",
        ),
        (
            with(
                r#"{"id":"q","request":{},"template_id":"t","ratings":[]}"#,
                r#"{"precision":{}}"#,
            ),
            "Ambiguous rated request",
        ),
        (
            with(
                r#"{"id":"q","params":{"a":1},"ratings":[]}"#,
                r#"{"precision":{}}"#,
            ),
            "need to set id of template",
        ),
        (
            with(
                r#"{"id":"q","template_id":"t","params":{"a":1},"ratings":[]}"#,
                r#"{"precision":{}}"#,
            ),
            "neither template nor evaluation request",
        ),
        (
            // ALLOC: test data.
                            r#"{"requests":[{"id":"q","template_id":"t","params":{"a":1},"ratings":[]}],"metric":{"precision":{}},"templates":[{"id":"y","template":"{}"}]}"#
            .to_owned(),
            "no template [t] for rated request [q]",
        ),
    ];
    for (body, reason) in cases {
        let e = parse(&body).expect_err(&body);
        assert!(e.contains(reason), "{body}: {e}");
    }
}

#[test]
fn ratings_and_templates_are_checked() {
    let rated = |ratings: &str| {
        with(
            // ALLOC: test data.
            &format!(r#"{{"id":"q","request":{{}},"ratings":[{ratings}]}}"#),
            r#"{"precision":{}}"#,
        )
    };
    let cases = [
        (
            rated(r#"{"_index":"","_id":"1","rating":1}"#),
            "Index must be set",
        ),
        (
            rated(r#"{"_index":"i","_id":"","rating":1}"#),
            "DocId must be set",
        ),
        (rated(r#"{"_id":"1","rating":1}"#), "_index is missing"),
        (rated(r#"{"_index":"i","rating":1}"#), "_id is missing"),
        (rated(r#"{"_index":"i","_id":"1"}"#), "rating is missing"),
        (
            rated(r#"{"_index":"i","_id":"1","rating":1,"x":1}"#),
            "unknown field [x]",
        ),
        (
            rated(r#"{"_index":"i","_id":"1","rating":"a"}"#),
            "rating doesn't support",
        ),
        (
            rated(r#"{"_index":"i","_id":1,"rating":1}"#),
            "_id doesn't support",
        ),
        (
            rated(r#"{"_index":"i","_id":"1","rating":1},{"_index":"i","_id":"1","rating":2}"#),
            r#"Found duplicate rated document key [{"_index":"i","_id":"1"}] in evaluation request [q]"#,
        ),
        (
            with(
                r#"{"id":"q","request":{},"ratings":[],"summary_fields":["a"]}"#,
                r#"{"precision":{}}"#,
            ),
            "summary_fields",
        ),
        (
            with(
                r#"{"id":"q","request":{},"ratings":[],"x":1}"#,
                r#"{"precision":{}}"#,
            ),
            "unknown field [x]",
        ),
    ];
    for (body, reason) in cases {
        let e = parse(&body).expect_err(&body);
        assert!(e.contains(reason), "{body}: {e}");
    }
    let templated = |template: &str| {
        // ALLOC: test data.
        format!(r#"{{"requests":[{REQ}],"metric":{{"precision":{{}}}},"templates":[{template}]}}"#)
    };
    for (template, reason) in [
        (r#"{"id":"x"}"#, "template is missing"),
        (r#"{"template":"a"}"#, "id is missing"),
        (r#"{"id":"x","template":1}"#, "template doesn't support"),
        (
            r#"{"id":"x","template":{"source":"a","lang":"painless"}}"#,
            "not supported by this node yet",
        ),
        (
            r#"{"id":"x","template":{"nope":1}}"#,
            "unknown field [nope]",
        ),
        (
            r#"{"id":"x","template":"a","nope":1}"#,
            "unknown field [nope]",
        ),
    ] {
        let e = parse(&templated(template)).expect_err(template);
        assert!(e.contains(reason), "{template}: {e}");
    }
}
