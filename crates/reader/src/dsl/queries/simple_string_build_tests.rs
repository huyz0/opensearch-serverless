//! Tests of `simple_query_string`'s grammar and leaf queries, built over a
//! mapping; compiled only under `#[cfg(test)]`. Each expectation was
//! compared with the query OpenSearch 3.10.0 builds for the same body
//! (`_validate/query?explain=true`); `show` writes what that prints, with
//! `+`, `-` and `%n` for the occurrences and the minimum should match.

use lucene_analysis::Analyzer;
use lucene_search::Clause;

use crate::dsl::parse_chunks;
use crate::dsl::queries::More;
use crate::dsl::queries::simple_string_build::simple_query_string;
use crate::dsl::query::Query;
use crate::failure::Failure;
use crate::lucene_query::QueryContext;
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"body":{"type":"text"},
    "tag":{"type":"keyword"},"year":{"type":"integer"}}}"#;

/// `show` of the query a `simple_query_string` body builds.
fn built(body: &str) -> Result<String, Failure> {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = QueryContext {
        mapping: &mapping,
        analyzer: &analyzer,
        max_doc: 5,
        knn: None,
        pinned: None,
    };
    let text = format!(r#"{{"query":{{"simple_query_string":{body}}}}}"#);
    let source = parse_chunks([text.as_bytes()]).expect("parses");
    let Some(Query::More(m)) = source.query else {
        panic!("not a simple_query_string");
    };
    let More::SimpleQueryString(s, _) = *m else {
        panic!("not a simple_query_string");
    };
    simple_query_string(&cx, &s).map(|c| show(&c))
}

fn ok(body: &str) -> String {
    built(body).expect("builds")
}

pub(super) fn show(c: &Clause) -> String {
    match c {
        Clause::Term(t) => format!("{}:{}", t.field, String::from_utf8_lossy(&t.term)),
        Clause::Prefix(p) => format!("{}:{}*", p.field, String::from_utf8_lossy(&p.prefix)),
        Clause::Fuzzy(f) => format!(
            "{}:{}~{}/{}/{}/{}",
            f.field,
            String::from_utf8_lossy(&f.term),
            f.max_edits,
            f.prefix_length,
            f.max_expansions,
            f.transpositions
        ),
        Clause::Phrase(p) => {
            let words: Vec<_> = p.terms.iter().map(|t| String::from_utf8_lossy(t)).collect();
            format!("{}:\"{}\"~{}", p.field, words.join(" "), p.slop)
        }
        Clause::Boost(b) => format!("{}^{}", show(&b.inner), b.boost),
        Clause::DisjunctionMax(d) => {
            let parts: Vec<_> = d.disjuncts.iter().map(show).collect();
            format!("dm{}({})", d.tie_breaker, parts.join("|"))
        }
        Clause::Boolean(b) => {
            let mut parts = Vec::new();
            parts.extend(b.must.iter().map(|c| format!("+{}", show(c))));
            parts.extend(b.should.iter().map(show));
            parts.extend(b.filter.iter().map(|c| format!("#{}", show(c))));
            parts.extend(b.must_not.iter().map(|c| format!("-{}", show(c))));
            let msm = if b.minimum_should_match > 0 {
                format!("%{}", b.minimum_should_match)
            } else {
                String::new()
            };
            format!("({}){msm}", parts.join(" "))
        }
        Clause::ConstantScore(c) => format!("cs({})", show(&c.inner)),
        Clause::MatchNoDocs(_) => "none".to_owned(),
        Clause::MatchAllDocs(_) => "all".to_owned(),
        other => format!("{other:?}"),
    }
}

fn title(query: &str) -> String {
    ok(&format!(r#"{{"query":"{query}","fields":["title"]}}"#))
}

#[test]
fn words_are_or_ed_or_and_ed_by_the_default_operator() {
    assert_eq!(title("quick"), "title:quick");
    assert_eq!(title("quick fox"), "(title:quick title:fox)");
    assert_eq!(
        ok(r#"{"query":"quick fox","fields":["title"],"default_operator":"and"}"#),
        "(+title:quick +title:fox)"
    );
}

#[test]
fn plus_pipe_and_minus_set_each_clause() {
    assert_eq!(title("quick + fox"), "(+title:quick +title:fox)");
    assert_eq!(title("quick | fox"), "(title:quick title:fox)");
    assert_eq!(
        ok(r#"{"query":"quick | fox","fields":["title"],"default_operator":"and"}"#),
        "(title:quick title:fox)"
    );
    assert_eq!(title("brown -sugar"), "(title:brown (all -title:sugar))");
    assert_eq!(title("-sugar"), "(all -title:sugar)");
    assert_eq!(title("a + b | c"), "((+title:a +title:b) title:c)");
    assert_eq!(title("a | b + c"), "(+(title:a title:b) +title:c)");
}

#[test]
fn groups_nest_and_unbalanced_ones_are_read_leniently() {
    assert_eq!(
        title("(quick | brown) + fox"),
        "(+(title:quick title:brown) +title:fox)"
    );
    assert_eq!(
        title("((quick) | (brown -sugar))"),
        "(title:quick (title:brown (all -title:sugar)))"
    );
    assert_eq!(title("(quick fox"), "(title:quick title:fox)");
    assert_eq!(title("quick fox)"), "(title:quick title:fox)");
    assert_eq!(title("()"), "none");
}

#[test]
fn a_group_nested_past_the_limit_is_refused() {
    let deep = format!("{}a{}", "(".repeat(129), ")".repeat(129));
    let e = built(&format!(r#"{{"query":"{deep}","fields":["title"]}}"#)).expect_err("refused");
    assert!(e.reason.contains("not supported"), "{}", e.reason);
    let fine = format!("{}a{}", "(".repeat(100), ")".repeat(100));
    assert_eq!(title(&fine), "title:a");
}

#[test]
fn phrases_take_their_slop() {
    assert_eq!(title(r#"\"brown fox\""#), r#"title:"brown fox"~0"#);
    assert_eq!(title(r#"\"quick fox\"~2"#), r#"title:"quick fox"~2"#);
    assert_eq!(title(r#"\"quick fox\"~"#), r#"title:"quick fox"~0"#);
    assert_eq!(title(r#"\"quick fox\"~x"#), r#"title:"quick fox"~0"#);
    assert_eq!(title(r#"\"solo\""#), "title:solo");
    assert_eq!(title(r#"\"brown fox"#), "(title:brown title:fox)");
    assert_eq!(
        title(r#"+\"brown fox\" -sugar"#),
        r#"(title:"brown fox"~0 (all -title:sugar))"#
    );
}

#[test]
fn prefix_is_lower_cased_by_the_field_analyzer() {
    assert_eq!(title("qui*"), "title:qui*");
    assert_eq!(title("QUI*"), "title:qui*");
    assert_eq!(ok(r#"{"query":"ani*","fields":["tag"]}"#), "tag:ani*");
    assert_eq!(ok(r#"{"query":"ANI*","fields":["tag"]}"#), "tag:ANI*");
}

#[test]
fn fuzziness_defaults_to_two_and_is_capped() {
    assert_eq!(title("quikc~1"), "title:quikc~1/0/50/true");
    assert_eq!(title("quikc~"), "title:quikc~2/0/50/true");
    assert_eq!(title("quikc~5"), "title:quikc~2/0/50/true");
    assert_eq!(title("quikc~0"), "title:quikc");
    assert_eq!(title("quikc~x"), "title:quikc");
    assert_eq!(
        ok(
            r#"{"query":"quikc~1","fields":["title"],"fuzzy_prefix_length":1,
            "fuzzy_max_expansions":7,"fuzzy_transpositions":false}"#
        ),
        "title:quikc~1/1/7/false"
    );
}

#[test]
fn flags_switch_each_feature_off() {
    let with = |flags: &str, q: &str| {
        ok(&format!(
            r#"{{"query":"{q}","fields":["title"],"flags":"{flags}"}}"#
        ))
    };
    assert_eq!(with("NONE", "quick | fox"), "(title:quick title:fox)");
    assert_eq!(with("NONE", "quick + fox"), "(title:quick title:fox)");
    assert_eq!(with("OR", "quick | fox"), "(title:quick title:fox)");
    assert_eq!(with("AND", "quick + fox"), "(+title:quick +title:fox)");
    assert_eq!(with("NOT", "brown -sugar"), "(title:brown title:sugar)");
    assert_eq!(
        with("NOT|WHITESPACE", "brown -sugar"),
        "(title:brown (all -title:sugar))"
    );
    assert_eq!(with("OR", "brown -sugar"), "(title:brown title:sugar)");
    assert_eq!(with("PREFIX", "qui*"), "title:qui*");
    assert_eq!(with("OR", "qui*"), "title:qui");
    assert_eq!(with("FUZZY", "quikc~1"), "title:quikc~1/0/50/true");
    assert_eq!(with("OR", "quikc~1"), "(title:quikc title:1)");
    assert_eq!(with("PHRASE", r#"\"brown fox\""#), r#"title:"brown fox"~0"#);
    assert_eq!(with("OR", r#"\"brown fox\""#), "(title:brown title:fox)");
    assert_eq!(
        with("PRECEDENCE", "(quick | brown) + fox"),
        "((title:quick title:brown) title:fox)"
    );
    assert_eq!(
        with("PRECEDENCE|OR|AND", "(quick | brown) + fox"),
        "(+(title:quick title:brown) +title:fox)"
    );
    assert_eq!(
        with("OR", "(quick | brown) + fox"),
        "(title:quick (title:brown title:fox))"
    );
    assert_eq!(
        with("NEAR", r#"\"quick fox\"~2"#),
        "(title:quick title:fox title:2)"
    );
    assert_eq!(
        with("PHRASE|NEAR", r#"\"quick fox\"~2"#),
        r#"title:"quick fox"~2"#
    );
    assert_eq!(
        with("PHRASE", r#"\"quick fox\"~2"#),
        r#"(title:"quick fox"~0 title:2)"#
    );
    assert_eq!(with("ALL", "qui* | fox"), "(title:qui* title:fox)");
    assert_eq!(
        with("WHITESPACE|PREFIX", "quick qui*"),
        "(title:quick title:qui*)"
    );
}

#[test]
fn flags_are_names_or_a_number() {
    let n = |flags: &str| {
        ok(&format!(
            r#"{{"query":"quick | fox","fields":["title"],"flags":{flags}}}"#
        ))
    };
    assert_eq!(n("0"), "(title:quick title:fox)");
    assert_eq!(n("-1"), "(title:quick title:fox)");
    assert_eq!(n("16"), "(title:quick title:fox)");
}

#[test]
fn escapes_make_a_character_part_of_a_word() {
    let tag = |q: &str, flags: &str| {
        ok(&format!(
            r#"{{"query":"{q}","fields":["tag"],"flags":"{flags}"}}"#
        ))
    };
    assert_eq!(tag(r"quick\\ fox", "ALL"), "cs(tag:quick fox)");
    assert_eq!(tag(r"quick\\+fox", "ALL"), "cs(tag:quick+fox)");
    assert_eq!(tag(r"ab\\", "ALL"), "cs(tag:ab)");
    assert_eq!(tag(r"quick\\ fox", "OR"), r"cs(tag:quick\ fox)");
    assert_eq!(tag("quick fox", "ALL"), "(cs(tag:quick) cs(tag:fox))");
    assert_eq!(tag("quick fox", "NONE"), "cs(tag:quick fox)");
}

#[test]
fn empty_and_star_queries() {
    assert_eq!(title(""), "none");
    assert_eq!(title("   "), "none");
    assert_eq!(title("*"), "all");
    assert_eq!(title("%"), "none");
    assert_eq!(title("%% fox"), "title:fox");
}

#[test]
fn several_fields_are_dis_maxed_with_a_tie_of_one() {
    let one = ok(r#"{"query":"fox","fields":["title","body"]}"#);
    assert!(
        one.starts_with("dm1(") && one.contains("title:fox") && one.contains("body:fox"),
        "{one}"
    );
    let two = ok(r#"{"query":"quick fox","fields":["title","body"]}"#);
    assert!(
        two.starts_with("(") && two.contains("title:quick") && two.contains("body:fox"),
        "{two}"
    );
    assert_eq!(ok(r#"{"query":"fox","fields":["title^3"]}"#), "title:fox^3");
    assert_eq!(
        ok(r#"{"query":"qui*","fields":["title^3"]}"#),
        "title:qui*^3"
    );
    assert_eq!(ok(r#"{"query":"fox","fields":["nothing"]}"#), "none");
}

#[test]
fn a_prefix_over_fields_is_a_dis_max_of_each() {
    let p = ok(r#"{"query":"qui*","fields":["title","body^2"]}"#);
    assert!(p.starts_with("dm1("), "{p}");
    assert!(p.contains("title:qui*") && p.contains("body:qui*^2"), "{p}");
}

#[test]
fn minimum_should_match_applies_to_a_top_level_boolean_only() {
    assert_eq!(
        ok(r#"{"query":"quick brown fox","fields":["title"],"minimum_should_match":"2"}"#),
        "(title:quick title:brown title:fox)%2"
    );
    assert_eq!(
        ok(r#"{"query":"quick brown fox snow","fields":["title"],"minimum_should_match":"50%"}"#),
        "(title:quick title:brown title:fox title:snow)%2"
    );
    assert_eq!(
        ok(r#"{"query":"quick","fields":["title"],"minimum_should_match":"2"}"#),
        "title:quick"
    );
    assert_eq!(
        ok(r#"{"query":"quick (brown fox)","fields":["title"],"minimum_should_match":"2"}"#),
        "(title:quick (title:brown title:fox))%2"
    );
}

#[test]
fn a_number_in_text_fails_unless_lenient() {
    assert!(!ok(r#"{"query":"2002","fields":["year"]}"#).is_empty());
    let e = built(r#"{"query":"fox","fields":["year"]}"#).expect_err("not a number");
    assert!(e.reason.contains("fox"), "{}", e.reason);
    assert_eq!(
        ok(r#"{"query":"fox","fields":["year"],"lenient":true}"#),
        "none"
    );
    assert!(ok(r#"{"query":"fox"}"#).contains("title:fox"));
    let e = built(r#"{"query":"fox","fields":["year"],"lenient":false}"#).expect_err("strict");
    assert!(e.reason.contains("fox"), "{}", e.reason);
}

#[test]
fn too_many_clauses_fail() {
    let words = "a ".repeat(1025);
    let e = built(&format!(r#"{{"query":"{words}","fields":["title"]}}"#)).expect_err("limit");
    assert!(e.reason.contains("1024"), "{}", e.reason);
    let words = "a ".repeat(1024);
    assert!(built(&format!(r#"{{"query":"{words}","fields":["title"]}}"#)).is_ok());
}

#[test]
fn groups_hold_escapes_and_empty_ones_vanish() {
    assert_eq!(
        title(r"(quick\\) fox) brown"),
        "((title:quick title:fox) title:brown)"
    );
    assert_eq!(title(r"(\\(a) b"), "(title:a title:b)");
    assert_eq!(title("() fox"), "title:fox");
    assert_eq!(title("fox () bar"), "(title:fox title:bar)");
    assert_eq!(title("-() fox"), "title:fox");
    assert_eq!(title("(a)b"), "(title:a title:b)");
    assert_eq!(title("(a b"), "(title:a title:b)");
    assert_eq!(title("a ( b"), "(title:a title:b)");
    let many = "(a) ".repeat(200);
    let q = ok(&format!(r#"{{"query":"{many}","fields":["title"]}}"#));
    assert_eq!(
        q.matches("title:a").count(),
        200,
        "a sibling group is not a deeper one"
    );
}

#[test]
fn a_phrase_ends_at_its_quote_and_a_slop_follows_it() {
    assert_eq!(title(r#"\"quick fox\"~2"#), r#"title:"quick fox"~2"#);
    assert_eq!(title(r#"\"quick fox\"~12"#), r#"title:"quick fox"~12"#);
    assert_eq!(
        title(r#"\"quick fox\"~2 x"#),
        r#"(title:"quick fox"~2 title:x)"#
    );
    assert_eq!(
        title(r#"\"quick fox\"~ x"#),
        r#"(title:"quick fox"~2 title:x)"#
    );
    assert_eq!(
        title(r#"\"quick fox\"x"#),
        r#"(title:"quick fox"~0 title:x)"#
    );
    assert_eq!(
        title(r#"\"a b\" c \"d e\""#),
        r#"(title:"a b"~0 title:c title:"d e"~0)"#
    );
    assert_eq!(title(r#"\"\" fox"#), "title:fox");
    assert_eq!(title(r#"\"\"\"a b\""#), r#"title:"a b"~0"#);
    assert_eq!(title(r#"\"a\\\" b\""#), r#"title:"a b"~0"#);
}

#[test]
fn a_token_needs_a_character_before_its_star_or_tilde() {
    assert_eq!(title("~1 fox"), "(title:1 title:fox)");
    assert_eq!(title("*x"), "title:x");
    assert_eq!(title("~"), "none");
    assert_eq!(title("x*y"), "(title:x title:y)");
    assert_eq!(title("x~1y"), "title:x");
    assert_eq!(title("x~12"), "title:x~2/0/50/true");
    assert_eq!(title("x~1 y~"), "(title:x~1/0/50/true title:y~2/0/50/true)");
}

#[test]
fn each_delimiter_ends_a_token() {
    assert_eq!(title(r#"a\"b\""#), "(title:a title:b)");
    assert_eq!(title("a|b"), "(title:a title:b)");
    assert_eq!(title("a+b"), "(+title:a +title:b)");
    assert_eq!(title("a(b)"), "(title:a title:b)");
    assert_eq!(title("a)b"), "(title:a title:b)");
    assert_eq!(title("a\\tb"), "(title:a title:b)");
}

#[test]
fn a_clause_that_fails_leniently_stays_and_one_without_terms_goes() {
    let year = |q: &str| {
        ok(&format!(
            r#"{{"query":"{q}","fields":["year"],"lenient":true}}"#
        ))
    };
    assert_eq!(year("fox bar"), "(none none)");
    assert_eq!(year("fox bar baz"), "(none none none)");
    assert_eq!(year("% fox"), "(none none)");
    assert_eq!(title("% fox"), "title:fox");
    assert_eq!(ok(r#"{"query":"%","fields":["tag"]}"#), "cs(tag:%)");
}

#[test]
fn an_operator_with_nothing_before_it_is_ignored() {
    assert_eq!(title("+a b"), "(title:a title:b)");
    assert_eq!(title("|a b"), "(title:a title:b)");
    assert_eq!(title("+ a"), "title:a");
    assert_eq!(title("- a"), "title:a");
    assert_eq!(title("a + | b"), "(+title:a +title:b)");
    assert_eq!(title("a | + b"), "(title:a title:b)");
    assert_eq!(title("x + (a | b"), "((+title:x +title:a) title:b)");
}

#[test]
fn a_lone_star_or_tilde_is_no_token_and_a_bracket_ends_a_keyword() {
    assert_eq!(title("a *"), "title:a");
    assert_eq!(title("a ~ b"), "(title:a title:b)");
    let tag = |q: &str| ok(&format!(r#"{{"query":"{q}","fields":["tag"]}}"#));
    assert_eq!(tag("a(b)"), "(cs(tag:a) cs(tag:b))");
    assert_eq!(tag("a)b"), "(cs(tag:a) cs(tag:b))");
}

#[test]
fn the_phrase_after_a_phrase_opens_at_its_own_quote() {
    assert_eq!(
        title(r#"\"\" x y \"a b\""#),
        r#"(title:x title:y title:"a b"~0)"#
    );
    assert_eq!(
        title(r#"\"a b\" c d \"e f\""#),
        r#"(title:"a b"~0 title:c title:d title:"e f"~0)"#
    );
}
