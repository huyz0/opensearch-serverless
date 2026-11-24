//! Tests of `query_string`'s leaf queries built over a mapping; compiled
//! only under `#[cfg(test)]`. Each expectation was compared with the query
//! OpenSearch 3.10.0 builds for the same body (`_validate/query?explain=true`);
//! `show` is `simple_string_build_tests`'s.

use lucene_analysis::Analyzer;

use super::simple_string_build_tests::show;
use crate::dsl::parse_chunks;
use crate::dsl::queries::More;
use crate::dsl::queries::query_string_build::query_string;
use crate::dsl::query::Query;
use crate::failure::Failure;
use crate::lucene_query::QueryContext;
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"body":{"type":"text"},
    "tag":{"type":"keyword"},"year":{"type":"integer"},"published":{"type":"date"}}}"#;

fn built(body: &str) -> Result<String, Failure> {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = QueryContext {
        mapping: &mapping,
        analyzer: &analyzer,
        max_doc: 5,
        knn: None,
    };
    let text = format!(r#"{{"query":{{"query_string":{body}}}}}"#);
    let source = match parse_chunks([text.as_bytes()]) {
        Ok(s) => s,
        Err(e) => return Err(Failure::plain("refused", format!("{e:?}"))),
    };
    let Some(Query::More(m)) = source.query else {
        panic!("not a query_string");
    };
    let More::QueryString(q, _) = *m else {
        panic!("not a query_string");
    };
    query_string(&cx, &q).map(|c| show(&c))
}

fn ok(body: &str) -> String {
    built(body).expect("builds")
}

fn title(query: &str) -> String {
    ok(&format!(r#"{{"query":"{query}","fields":["title"]}}"#))
}

fn both(query: &str) -> String {
    ok(&format!(
        r#"{{"query":"{query}","fields":["title","tag"]}}"#
    ))
}

fn fails(body: &str) -> Failure {
    built(body).expect_err("fails")
}

#[test]
fn a_word_is_matched_over_the_fields_by_the_type() {
    assert_eq!(title("fox"), "title:fox");
    assert_eq!(both("fox"), "dm0(title:fox|cs(tag:fox))");
    assert_eq!(
        ok(r#"{"query":"fox","fields":["title","tag"],"type":"most_fields"}"#),
        "dm1(title:fox|cs(tag:fox))"
    );
    assert_eq!(
        ok(r#"{"query":"fox","fields":["title","tag"],"tie_breaker":0.3}"#),
        "dm0.3(title:fox|cs(tag:fox))"
    );
    assert_eq!(
        ok(r#"{"query":"fox","fields":["title^2","tag"]}"#),
        "dm0(title:fox^2|cs(tag:fox))"
    );
    assert_eq!(
        ok(r#"{"query":"fox","default_field":"title"}"#),
        "title:fox"
    );
    assert_eq!(ok(r#"{"query":"fox","default_field":"ti*"}"#), "title:fox");
    assert_eq!(ok(r#"{"query":"fox","fields":["ti*"]}"#), "title:fox");
}

#[test]
fn several_words_are_one_match_a_field() {
    assert_eq!(
        both("fox quick"),
        "dm0((title:fox title:quick)|cs(tag:fox quick))"
    );
    assert_eq!(
        ok(r#"{"query":"quick fox","fields":["title"],"default_operator":"and"}"#),
        "(+title:quick +title:fox)"
    );
    assert_eq!(
        ok(r#"{"query":"quick fox","fields":["title","tag"],"type":"phrase"}"#),
        r#"dm0(title:"quick fox"~0|cs(tag:quick fox))"#
    );
}

#[test]
fn every_field_is_searched_by_default_and_leniently() {
    let all = ok(r#"{"query":"fox"}"#);
    assert_eq!(all, "dm0(body:fox|none|cs(tag:fox)|title:fox|none)");
    assert_eq!(ok(r#"{"query":"fox","default_field":"*"}"#), all);
    assert_eq!(ok(r#"{"query":"*:fox"}"#), all);
    assert!(ok(r#"{"query":"2002"}"#).contains("title:2002"));
}

#[test]
fn operators_and_groups_follow_lucenes_grammar() {
    assert_eq!(title("the AND fox"), "(+title:the +title:fox)");
    assert_eq!(title("+quick -fox"), "(+title:quick -title:fox)");
    assert_eq!(title("quick NOT fox"), "(title:quick -title:fox)");
    assert_eq!(
        title("fox AND (quick OR NOT lazy)"),
        "(+title:fox +(title:quick -title:lazy))"
    );
    assert_eq!(
        ok(r#"{"query":"title:(quick fox)"}"#),
        "(title:quick title:fox)"
    );
    assert_eq!(
        ok(r#"{"query":"title:fox AND tag:animal"}"#),
        "(+title:fox +cs(tag:animal))"
    );
}

#[test]
fn a_query_of_only_prohibited_clauses_also_matches_everything() {
    assert_eq!(title("-fox"), "(#all -title:fox)");
    assert_eq!(title("NOT fox"), "(#all -title:fox)");
    assert_eq!(title("(-fox)"), "(#all -title:fox)");
    assert_eq!(title("a (-fox)"), "(title:a (#all -title:fox))");
}

#[test]
fn empty_blank_and_star_queries() {
    assert_eq!(title(""), "none");
    assert_eq!(title("   "), "none");
    assert_eq!(ok(r#"{"query":"*:*","fields":["title"]}"#), "all");
    assert_eq!(ok(r#"{"query":"*"}"#), "all");
    assert!(title("*").starts_with("cs(Exists"));
    assert_eq!(
        both("*"),
        "dm0(cs(Exists(FieldExistsQuery { field: \"title\" }))|cs(Exists(FieldExistsQuery { field: \"tag\" })))"
    );
    assert!(ok(r#"{"query":"title:*","fields":["tag"]}"#).contains("title"));
}

#[test]
fn phrases_take_their_slop_from_the_text_or_the_setting() {
    assert_eq!(title("\\\"brown fox\\\""), "title:\"brown fox\"~0");
    assert_eq!(title("\\\"brown fox\\\"~2"), "title:\"brown fox\"~2");
    assert_eq!(
        ok(r#"{"query":"\"brown fox\"","fields":["title"],"phrase_slop":3}"#),
        "title:\"brown fox\"~3"
    );
    assert_eq!(title("\\\"brown\\\""), "title:brown");
    assert_eq!(
        ok(r#"{"query":"\"brown fox\"~2","fields":["title","tag"]}"#),
        "dm0(title:\"brown fox\"~2|cs(tag:brown fox))"
    );
    assert_eq!(title("\\\"brown fox\\\"~2^3"), "title:\"brown fox\"~2^3");
}

#[test]
fn a_phrase_with_a_negative_slop_is_refused() {
    let f = fails(r#"{"query":"\"a b\"","fields":["title"],"phrase_slop":-1}"#);
    assert_eq!(f.reason, "slop value cannot be negative");
}

#[test]
fn wildcards_are_lower_cased_on_text_fields_only() {
    assert_eq!(
        both("F*x"),
        "dm0(Wildcard(WildcardQuery { field: \"title\", pattern: [102, 42, 120] })|\
         Wildcard(WildcardQuery { field: \"tag\", pattern: [70, 42, 120] }))"
    );
    assert_eq!(title("fox*"), "title:fox*");
    assert_eq!(title("A\\\\*B*"), "title:a*b*");
    assert!(title("A\\\\*b?").contains("[97, 92, 42, 98, 63]"));
    assert_eq!(
        ok(r#"{"query":"f*","fields":["title^3","tag"]}"#),
        "dm0(title:f*^3|tag:f*)"
    );
}

#[test]
fn a_leading_wildcard_is_refused_when_not_allowed() {
    assert!(title("*ox").contains("Wildcard"));
    for q in ["*ox", "?ox", "*ox*"] {
        let f = fails(&format!(
            r#"{{"query":"{q}","fields":["title"],"allow_leading_wildcard":false}}"#
        ));
        assert_eq!(f.kind, "query_shard_exception", "{q}");
        assert_eq!(f.reason, format!("Failed to parse query [{q}]"));
        let cause = f.cause.as_ref().expect("cause");
        assert_eq!(
            cause.reason,
            format!(
                "Cannot parse '{q}': '*' or '?' not allowed as first character in WildcardQuery"
            )
        );
    }
    let f = fails(r#"{"query":"*ox","fields":["title"],"allow_leading_wildcard":false}"#);
    let inner = f
        .cause
        .as_ref()
        .and_then(|c| c.cause.as_ref())
        .expect("inner");
    assert_eq!(
        inner.reason,
        "'*' or '?' not allowed as first character in WildcardQuery"
    );
}

#[test]
fn a_leading_wildcard_check_fails_before_the_field_is_looked_up_for_prefix() {
    // A prefix has no leading `*` after the grammar takes it apart, so the
    // check is on the term the hook is given.
    assert_eq!(title("qui*"), "title:qui*");
}

#[test]
fn prefixes_and_regexps_follow_the_field_type() {
    assert_eq!(title("QUI*"), "title:qui*");
    assert_eq!(ok(r#"{"query":"ANI*","fields":["tag"]}"#), "tag:ANI*");
    assert!(title("/F.x/").contains("pattern: \"f.x\""));
    assert!(ok(r#"{"query":"/F.x/","fields":["tag"]}"#).contains("pattern: \"F.x\""));
}

#[test]
fn an_invalid_regexp_fails_the_query() {
    let f = fails(r#"{"query":"/F(/","fields":["title"]}"#);
    assert_eq!(f.kind, "illegal_argument_exception");
    assert_eq!(f.reason, "unexpected end-of-string");
}

#[test]
fn string_queries_on_other_types_fail_unless_lenient() {
    for (q, what) in [("fo?", "wildcard"), ("fox*", "prefix"), ("/f.x/", "regexp")] {
        let f = fails(&format!(r#"{{"query":"{q}","fields":["year"]}}"#));
        assert_eq!(
            f.reason,
            format!(
                "Can only use {what} queries on keyword and text fields - not on [year] which is of type [integer]"
            ),
            "{q}"
        );
        assert_eq!(
            ok(&format!(
                r#"{{"query":"{q}","fields":["year"],"lenient":true}}"#
            )),
            "none"
        );
    }
}

#[test]
fn fuzzy_distance_is_the_number_after_the_tilde() {
    assert_eq!(title("fox~"), "title:fox~1/0/50/true");
    assert_eq!(title("fox~1"), "title:fox~1/0/50/true");
    assert_eq!(title("fox~0"), "title:fox~0/0/50/true");
    assert_eq!(title("fox~0.5"), "title:fox~0/0/50/true");
    assert_eq!(title("fox~1.5"), "title:fox~1/0/50/true");
    assert_eq!(title("fox~3"), "title:fox~2/0/50/true");
    assert_eq!(title("fox~2"), "title:fox~2/0/50/true");
    assert_eq!(title("fox~2f"), "title:fox~2/0/50/true");
    assert_eq!(title("QUIKC~1"), "title:quikc~1/0/50/true");
    assert_eq!(
        both("fox~"),
        "dm0(title:fox~1/0/50/true|tag:fox~1/0/50/true)"
    );
}

#[test]
fn a_bare_tilde_takes_its_distance_from_fuzziness() {
    assert_eq!(
        ok(r#"{"query":"fox~","fields":["title"],"fuzziness":1}"#),
        "title:fox~1/0/50/true"
    );
    assert_eq!(
        ok(r#"{"query":"fox~","fields":["title"],"fuzziness":"AUTO:1,2"}"#),
        "title:fox~2/0/50/true"
    );
    assert_eq!(
        ok(r#"{"query":"fox~2","fields":["title"],"fuzziness":0}"#),
        "title:fox~2/0/50/true"
    );
    assert_eq!(
        ok(r#"{"query":"quikc~2","fields":["title"],"fuzzy_prefix_length":2}"#),
        "title:quikc~2/2/50/true"
    );
}

#[test]
fn a_fuzzy_distance_that_is_not_a_number_fails() {
    let f = fails(r#"{"query":"fox~abc","fields":["title"]}"#);
    assert_eq!(f.kind, "number_format_exception");
    assert_eq!(f.reason, "For input string: \"ABC\"");
}

#[test]
fn numeric_ranges_are_open_at_a_star_and_a_comparison() {
    let range = |q: &str| ok(&format!(r#"{{"query":"{q}"}}"#));
    assert!(range("year:[2001 TO 2005]").contains("[128, 0, 7, 209], upper: [128, 0, 7, 213]"));
    assert!(range("year:{2001 TO 2005]").contains("[128, 0, 7, 210], upper: [128, 0, 7, 213]"));
    assert!(range("year:[* TO 2005]").contains("lower: [0, 0, 0, 0], upper: [128, 0, 7, 213]"));
    assert!(range("year:>2002").contains("[128, 0, 7, 211], upper: [255, 255, 255, 255]"));
    assert!(range("year:>=2002").contains("[128, 0, 7, 210], upper: [255, 255, 255, 255]"));
    assert!(range("year:<2002").contains("lower: [0, 0, 0, 0], upper: [128, 0, 7, 209]"));
    assert!(range("year:<=2002").contains("lower: [0, 0, 0, 0], upper: [128, 0, 7, 210]"));
    assert!(range("year:[2001 TO 2002]^2").ends_with("^2"));
    assert!(range("published:[2001 TO 2002]").contains("bytes_per_dim: 8"));
}

#[test]
fn a_comparison_needs_a_field_and_a_word_after_the_sign() {
    // No field of the clause's own: the sign is part of the word.
    let f = fails(r#"{"query":">2002","fields":["year"]}"#);
    assert_eq!(f.reason, "For input string: \">2002\"");
    assert!(ok(r#"{"query":">2002","default_field":"year"}"#).contains("upper: [255"));
    // A sign alone is a word; "=" follows ">" when nothing does after it.
    assert_eq!(ok(r#"{"query":"year:>"}"#), "none");
    let f = fails(r#"{"query":"year:>=","fields":["year"]}"#);
    assert_eq!(f.reason, "For input string: \"=\"");
    // A quoted sign is a phrase.
    let f = fails(r#"{"query":"year:\">2002\"","fields":["year"]}"#);
    assert_eq!(f.reason, "For input string: \">2002\"");
}

#[test]
fn a_range_of_strings_is_refused() {
    let f = fails(r#"{"query":"title:[a TO b]"}"#);
    assert!(
        f.reason.contains("is not supported by this node yet"),
        "{}",
        f.reason
    );
    let f = fails(r#"{"query":"title:>fox"}"#);
    assert!(f.reason.contains("range query"), "{}", f.reason);
}

#[test]
fn a_bad_number_fails_unless_lenient() {
    assert_eq!(ok(r#"{"query":"year:abc"}"#), "none");
    assert_eq!(ok(r#"{"query":"year:[a TO b]"}"#), "none");
    let f = fails(r#"{"query":"year:[a TO b]","fields":["year"]}"#);
    assert_eq!(f.reason, "For input string: \"a\"");
    let f = fails(r#"{"query":"fox","fields":["year","title"]}"#);
    assert_eq!(f.reason, "For input string: \"fox\"");
    assert_eq!(
        ok(r#"{"query":"fox","fields":["year"],"lenient":true}"#),
        "none"
    );
}

#[test]
fn unmapped_fields_match_nothing() {
    assert_eq!(ok(r#"{"query":"nothing:fox"}"#), "none");
    assert_eq!(ok(r#"{"query":"fox","fields":["nothing"]}"#), "none");
    assert_eq!(ok(r#"{"query":"nothing:fox*"}"#), "none");
    assert_eq!(ok(r#"{"query":"nothing:fox~"}"#), "none");
    assert_eq!(ok(r#"{"query":"nothing:\"a b\""}"#), "none");
    assert_eq!(ok(r#"{"query":"nothing:[1 TO 2]"}"#), "none");
}

#[test]
fn fields_and_a_default_field_together_are_refused() {
    let f = fails(r#"{"query":"fox","fields":["title"],"default_field":"title"}"#);
    assert_eq!(f.kind, "query_validation_exception");
    assert_eq!(
        f.reason,
        "Validation Failed: 1: [query_string] cannot use [fields] parameter in conjunction with [default_field];"
    );
}

#[test]
fn minimum_should_match_applies_to_a_top_level_boolean_only() {
    assert_eq!(
        ok(r#"{"query":"a b c","fields":["title"],"minimum_should_match":"2"}"#),
        "(title:a title:b title:c)%2"
    );
    assert_eq!(
        ok(r#"{"query":"a OR b OR c","fields":["title"],"minimum_should_match":"2"}"#),
        "(title:a title:b title:c)%2"
    );
    assert_eq!(
        ok(r#"{"query":"a","fields":["title"],"minimum_should_match":"2"}"#),
        "title:a"
    );
}

#[test]
fn escape_takes_the_text_literally() {
    assert_eq!(
        ok(r#"{"query":"fox bar","fields":["title"],"escape":true}"#),
        "(title:fox title:bar)"
    );
    assert_eq!(
        ok(r#"{"query":"fox:bar","fields":["title"],"escape":true}"#),
        "title:fox:bar"
    );
    assert_eq!(title("fox\\\\:bar"), "title:fox:bar");
}

#[test]
fn a_word_with_no_terms_is_dropped_and_a_phrase_too() {
    assert_eq!(title("\\\"!!!\\\" fox"), "(title:fox)");
    assert_eq!(title("\\\"!!!\\\""), "()");
}

#[test]
fn a_parse_error_is_a_query_shard_exception_with_the_parsers_cause() {
    let f = fails(r#"{"query":"(quick fox","fields":["title"]}"#);
    assert_eq!(f.kind, "query_shard_exception");
    assert_eq!(f.reason, "Failed to parse query [(quick fox]");
    let cause = f.cause.expect("cause");
    assert_eq!(cause.kind, "parse_exception");
    assert!(
        cause
            .reason
            .starts_with("Cannot parse '(quick fox': Encountered")
    );
    assert!(cause.cause.is_some());
}

#[test]
fn a_star_field_in_the_query_searches_every_field_even_with_a_list() {
    let q = ok(r#"{"query":"*:fox","fields":["title"],"lenient":true}"#);
    assert!(q.contains("tag:fox") && q.contains("title:fox"), "{q}");
    assert_eq!(ok(r#"{"query":"title:fox","fields":["tag"]}"#), "title:fox");
}

#[test]
fn the_tie_breaker_of_a_per_field_query_is_the_types_or_the_setting() {
    assert!(
        ok(r#"{"query":"f*","fields":["title","tag"],"type":"most_fields"}"#).starts_with("dm1(")
    );
    assert!(ok(r#"{"query":"f*","fields":["title","tag"]}"#).starts_with("dm0("));
    assert!(
        ok(r#"{"query":"f*","fields":["title","tag"],"tie_breaker":0.3}"#).starts_with("dm0.3(")
    );
}

#[test]
fn an_escaped_star_makes_a_prefix_with_a_leading_star() {
    let f = fails(r#"{"query":"\\*a*","fields":["title"],"allow_leading_wildcard":false}"#);
    assert_eq!(f.kind, "query_shard_exception");
    assert_eq!(
        f.cause.expect("cause").reason,
        "Cannot parse '\\*a*': '*' not allowed as first character in PrefixQuery"
    );
    assert_eq!(ok(r#"{"query":"\\*a*","fields":["title"]}"#), "title:*a*");
}

#[test]
fn a_bad_regexp_on_a_number_field_fails_on_the_type() {
    let f = fails(r#"{"query":"/F(/","fields":["year"]}"#);
    assert!(
        f.reason.starts_with("Can only use regexp queries"),
        "{}",
        f.reason
    );
}
