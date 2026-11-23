//! fixtures/mustache/render.json: what mustache.java renders for each case,
//! recorded with `fixtures/mustache/record.sh`, replayed through the port.

use super::{TemplateError, TemplateRequest, Val, read_val, render};
use crate::dsl::source::tokenizer;
use crate::dsl::{MAX_RECORDED_BYTES, Recorder};

fn parse(text: &str) -> Val {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokens = tokenizer();
    tokens.feed(text.as_bytes(), &mut rec).expect("json");
    tokens.finish(&mut rec).expect("json");
    let mut p = rec.parser();
    p.next_token();
    read_val(&mut p, 0).expect("a value")
}

fn text<'a>(case: &'a Val, key: &str) -> Option<&'a str> {
    match case.get(key) {
        Some(Val::Str(s)) => Some(s),
        _ => None,
    }
}

#[test]
fn every_recorded_case_renders_as_the_java_does() {
    let cases = parse(include_str!("../../../../fixtures/mustache/render.json"));
    let Val::List(cases) = cases else {
        panic!("a list of cases");
    };
    assert!(cases.len() > 100, "{} cases", cases.len());
    // ALLOC: test data.
    let empty = Val::Map(Vec::new());
    // ALLOC: test data.
    let mut wrong = Vec::new();
    for case in &cases {
        let template = text(case, "t").expect("a template");
        let params = case.get("p").unwrap_or(&empty);
        let got = render(template, params, 1 << 20);
        let want_out = text(case, "out");
        let want_err = text(case, "err");
        let ok = match (&got, want_out, want_err) {
            (Ok(out), Some(want), _) => out == want,
            (
                Err(
                    TemplateError::Compile(m)
                    | TemplateError::Runtime(m)
                    | TemplateError::Illegal(m),
                ),
                _,
                Some(err),
            ) => err.split_once(": ").map(|(_, message)| message) == Some(m.as_str()),
            _ => false,
        };
        if !ok {
            // ALLOC: test data.
            wrong.push(format!(
                "template {template:?} params {params:?}\n  want {:?}\n  got  {got:?}",
                want_out.map(str::to_owned).or(want_err.map(str::to_owned))
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} differ:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

fn unsupported(template: &str, params: &str) -> String {
    match render(template, &parse(params), 1 << 20) {
        Err(TemplateError::Unsupported(m)) => m,
        other => panic!("{template:?}: {other:?}"),
    }
}

#[test]
fn what_the_java_has_and_this_node_lacks_is_refused() {
    for template in [
        "{{$x}}d{{/x}}",
        "{{?x}}d{{/x}}",
        "{{%x}}",
        "{{*x}}",
        "{{#*x}}d{{/*x}}",
        "{{^*x}}d{{/*x}}",
        "{{{*x}}}",
        "{{&*x}}",
        "{{#toJson}}*x{{/toJson}}",
        "{{>*x}}",
        "{{#url}}{{*x}}{{/url}}",
    ] {
        let m = unsupported(template, "{}");
        assert!(
            m.ends_with("is not supported by this node yet"),
            "{template}: {m}"
        );
    }
    // A scalar scope has the members of its Java class.
    for (template, params) in [
        ("{{#s}}{{length}}{{/s}}", r#"{"s":"abc"}"#),
        ("{{#s}}{{empty}}{{/s}}", r#"{"s":"abc"}"#),
        ("{{#n}}{{intValue}}{{/n}}", r#"{"n":5}"#),
        ("{{#n}}{{longValue}}{{/n}}", r#"{"n":5000000000}"#),
        ("{{#n}}{{signum}}{{/n}}", r#"{"n":18446744073709551616}"#),
        ("{{#d}}{{infinite}}{{/d}}", r#"{"d":1.5}"#),
        ("{{#b}}{{booleanValue}}{{/b}}", r#"{"b":true}"#),
        ("{{#s}}{{length.x}}{{/s}}", r#"{"s":"abc"}"#),
        ("{{#l.size}}{{intValue}}{{/l.size}}", r#"{"l":[1]}"#),
    ] {
        let m = unsupported(template, params);
        assert!(
            m.contains("scope is not supported by this node yet"),
            "{template}: {m}"
        );
    }
}

#[test]
fn a_template_that_nests_too_deep_or_renders_too_much_is_refused() {
    // ALLOC: test data.
    let deep = format!("{}{}", "{{#a}}".repeat(300), "{{/a}}".repeat(300));
    let m = unsupported(&deep, r#"{"a":true}"#);
    assert!(m.contains("nested more than"), "{m}");
    let params = parse(r#"{"l":[1,2,3,4,5,6,7,8,9,10]}"#);
    assert_eq!(render("{{#l}}x{{/l}}", &params, 10), Ok("x".repeat(10)));
    assert_eq!(
        render("{{#l}}xx{{/l}}", &params, 10),
        Err(TemplateError::TooLarge(10))
    );
}

fn request(body: &str) -> Result<TemplateRequest, String> {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokens = tokenizer();
    tokens
        .feed(body.as_bytes(), &mut rec)
        // ALLOC: test data.
        .map_err(|e| format!("{e:?}"))?;
    // ALLOC: test data.
    tokens.finish(&mut rec).map_err(|e| format!("{e:?}"))?;
    TemplateRequest::parse(&rec).map_err(|e| e.into_failure().reason)
}

#[test]
fn a_request_names_its_template_and_params() {
    let r = request(r#"{"source":"{{a}}","params":{"a":1,"b":[true,null,"x",1.5,12345678901234567890]},"explain":true}"#)
        .expect("parses");
    assert_eq!(r.source.as_deref(), Some("{{a}}"));
    assert!(r.explain && !r.profile && !r.search_pipeline && r.id.is_none());
    let params = r.params.expect("params");
    assert_eq!(params.get("a"), Some(&Val::Int(1)));
    assert_eq!(
        params.get("b"),
        // ALLOC: test data.
        Some(&Val::List(vec![
            Val::Bool(true),
            Val::Null,
            Val::Str("x".into()),
            Val::Double(1.5),
            Val::Big("12345678901234567890".into()),
        ]))
    );
    for key in ["source", "inline", "template"] {
        // ALLOC: test data.
        let r = request(&format!(r#"{{"{key}":"t"}}"#)).expect("parses");
        assert_eq!(r.source.as_deref(), Some("t"), "{key}");
    }
    let r = request(r#"{"id":"stored","profile":true,"search_pipeline":"p"}"#).expect("parses");
    assert_eq!(r.id.as_deref(), Some("stored"));
    assert!(r.profile && r.search_pipeline && r.source.is_none());
    let r = request(r#"{"search_pipeline":null}"#).expect("parses");
    assert!(!r.search_pipeline);
    let r = request(r#"{"search_pipeline":""}"#).expect("parses");
    assert!(!r.search_pipeline);
}

#[test]
fn an_object_template_is_written_as_compact_json() {
    let r = request(
        r#"{"source":{ "query" : { "match": {"f":"{{q}}"} }, "n": [1, 2.50, 1e3, -0, "\u00e9"] }}"#,
    )
    .expect("parses");
    assert_eq!(
        r.source.as_deref(),
        Some(r#"{"query":{"match":{"f":"{{q}}"}},"n":[1,2.5,1000.0,0,"é"]}"#)
    );
}

#[test]
fn a_repeated_param_keeps_its_place_and_takes_the_last_value() {
    let r = request(r#"{"params":{"a":1,"b":2,"a":3}}"#).expect("parses");
    assert_eq!(
        r.params,
        // ALLOC: test data.
        Some(Val::Map(vec![
            ("a".into(), Val::Int(3)),
            ("b".into(), Val::Int(2))
        ]))
    );
}

#[test]
fn a_request_the_object_parser_refuses_says_why() {
    for (body, want) in [
        ("", "request body or source parameter is required"),
        (
            "[]",
            "[1:1] [search_template] Expected START_OBJECT but was: START_ARRAY",
        ),
        (
            r#"{"nope":1}"#,
            "[1:2] [search_template] unknown field [nope]",
        ),
        (
            r#"{"params":"x"}"#,
            "[1:11] [search_template] params doesn't support values of type: VALUE_STRING",
        ),
        (
            r#"{"id":1}"#,
            "[1:7] [search_template] id doesn't support values of type: VALUE_NUMBER",
        ),
        (
            r#"{"source":[1]}"#,
            "[1:11] [search_template] source doesn't support values of type: START_ARRAY",
        ),
        (
            r#"{"explain":"true"}"#,
            "[1:12] [search_template] explain doesn't support values of type: VALUE_STRING",
        ),
        (
            r#"{"search_pipeline":1}"#,
            "[1:20] [search_template] search_pipeline doesn't support values of type: VALUE_NUMBER",
        ),
    ] {
        assert_eq!(request(body).err().as_deref(), Some(want), "{body}");
    }
}

#[test]
fn params_nested_too_deep_are_refused() {
    // ALLOC: test data.
    let deep = format!(
        r#"{{"params":{{"a":{}1{}}}}}"#,
        "[".repeat(300),
        "]".repeat(300)
    );
    let e = request(&deep).expect_err("refused");
    assert!(e.contains("is not supported by this node yet"), "{e}");
}
