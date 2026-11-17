//! Tests of the `highlight` parser; compiled only under `#[cfg(test)]`. The
//! messages were answered by stock OpenSearch 3.10.0, which the compat
//! fixtures of `match.json` replay end to end.

use super::*;
use crate::dsl::source::tokenizer;
use crate::dsl::tokens::Recorder;

fn parse(highlight: &str) -> Result<HighlightSpec, String> {
    let body = format!(r#"{{"highlight":{highlight}}}"#);
    let mut rec = Recorder::new(1 << 16);
    let mut tokens = tokenizer();
    tokens.feed(body.as_bytes(), &mut rec).expect("tokens");
    tokens.finish(&mut rec).expect("tokens");
    let mut p = rec.parser();
    p.next_token();
    p.next_token();
    p.next_token();
    parse_highlight(&mut p).map_err(|e| e.into_failure().reason)
}

fn refusal(highlight: &str) -> String {
    parse(highlight).expect_err("refused")
}

#[test]
fn the_defaults_are_those_of_opensearch() {
    let spec = parse(r#"{"fields":{"a":{}}}"#).expect("parses");
    assert_eq!(
        spec.fields,
        [(
            "a".to_owned(),
            Options {
                pre_tag: "<em>".into(),
                post_tag: "</em>".into(),
                fragment_size: 100,
                number_of_fragments: 5,
                no_match_size: 0,
                score_ordered: false,
                html: false,
                require_field_match: true,
                word_boundary: false,
            }
        )]
    );
    assert!(parse("{}").expect("parses").fields.is_empty());
}

#[test]
fn a_field_option_beats_the_highlights_which_beats_the_default() {
    let spec = parse(
        r#"{"pre_tags":["<i>","<x>"],"post_tags":["</i>"],"fragment_size":7,"number_of_fragments":"2",
            "no_match_size":3,"order":"score","encoder":"html","require_field_match":false,
            "boundary_scanner":"word","type":"unified",
            "fields":{"a":{"pre_tags":["<u>"],"post_tags":["</u>"],"fragment_size":9,
                           "order":"none","encoder":"default","boundary_scanner":"sentence"},
                      "b":{}}}"#,
    )
    .expect("parses");
    let [(a, oa), (b, ob)] = &spec.fields[..] else {
        panic!("{spec:?}")
    };
    assert_eq!((a.as_str(), b.as_str()), ("a", "b"));
    assert_eq!((oa.pre_tag.as_str(), oa.post_tag.as_str()), ("<u>", "</u>"));
    assert_eq!((ob.pre_tag.as_str(), ob.post_tag.as_str()), ("<i>", "</i>"));
    assert_eq!((oa.fragment_size, ob.fragment_size), (9, 7));
    assert_eq!((oa.number_of_fragments, ob.number_of_fragments), (2, 2));
    assert_eq!((oa.no_match_size, ob.no_match_size), (3, 3));
    assert_eq!((oa.score_ordered, ob.score_ordered), (false, true));
    assert_eq!((oa.html, ob.html), (false, true));
    assert_eq!(
        (oa.require_field_match, ob.require_field_match),
        (false, false)
    );
    assert_eq!((oa.word_boundary, ob.word_boundary), (false, true));
}

#[test]
fn unknown_keys_fail_as_opensearch_fails_them() {
    assert_eq!(
        refusal(r#"{"nope":1}"#),
        "[1:15] [highlight] unknown field [nope]"
    );
    assert_eq!(
        refusal(r#"{"fragment_siz":1}"#),
        "[1:15] [highlight] unknown field [fragment_siz] did you mean [fragment_size]?"
    );
    assert_eq!(
        refusal(r#"{"nope":{}}"#),
        "[1:15] [highlight] unknown field [nope]"
    );
    assert_eq!(
        refusal(r#"{"fields":{"a":{"nope":1}}}"#),
        "[1:37] [highlight] failed to parse field [fields]"
    );
    assert_eq!(
        refusal(r#"{"fields":{"a":{"fragment_size":"x"}}}"#),
        "[1:46] [highlight] failed to parse field [fields]"
    );
}

#[test]
fn what_is_not_served_yet_is_refused() {
    for key in [
        "fragmenter",
        "force_source",
        "highlight_query",
        "highlight_filter",
        "boundary_chars",
        "boundary_max_scan",
        "boundary_scanner_locale",
        "tags_schema",
        "matched_fields",
        "max_analyzer_offset",
        "phrase_limit",
        "options",
        "fragment_offset",
    ] {
        for body in [
            format!(r#"{{"{key}":1}}"#),
            format!(r#"{{"fields":{{"a":{{"{key}":1}}}}}}"#),
        ] {
            assert_eq!(
                refusal(&body),
                format!("the highlight's [{key}] is not supported by this node yet"),
                "{body}"
            );
        }
    }
    for (body, what) in [
        (r#"{"type":"plain"}"#, "[type] of this kind"),
        (r#"{"fragment_size":-1}"#, "[fragment_size] of this kind"),
        (r#"{"no_match_size":null}"#, "[no_match_size] of this kind"),
        (r#"{"order":"x"}"#, "[order] of this kind"),
        (r#"{"encoder":"x"}"#, "[encoder] of this kind"),
        (
            r#"{"boundary_scanner":"chars"}"#,
            "[boundary_scanner] of this kind",
        ),
        (r#"{"order":{}}"#, "[order] of this kind"),
        (r#"{"fields":[]}"#, "[fields] of this kind"),
        (r#"{"fields":{"a":[]}}"#, "[fields] entry of this kind"),
        (r#"{"fields":{"a":1}}"#, "[fields] entry of this kind"),
        (
            r#"{"pre_tags":["<b>"]}"#,
            "[pre_tags] without [post_tags], or the reverse",
        ),
        (
            r#"{"post_tags":["<b>"]}"#,
            "[pre_tags] without [post_tags], or the reverse",
        ),
        (r#"{"pre_tags":[],"post_tags":[]}"#, "an empty [pre_tags]"),
        (
            r#"{"pre_tags":[1],"post_tags":["a"]}"#,
            "[pre_tags] entry of this kind",
        ),
    ] {
        assert_eq!(
            refusal(body),
            format!("the highlight's {what} is not supported by this node yet"),
            "{body}"
        );
    }
}
