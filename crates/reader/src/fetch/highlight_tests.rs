//! Tests of `highlight`; compiled only under `#[cfg(test)]`. The expected
//! fragments and messages were answered by stock OpenSearch 3.10.0, which
//! the compat fixtures of `match.json` replay end to end.

use super::*;
use crate::fetch::highlight_parse::parse_highlight;
use lucene_search::{Clause, TermQuery};
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use crate::{OpenShard, open_shard};

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let spec = Spec::new("ex", 20).segments_of(10).with_keywords(2);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "ex", 0).expect("opens")
}

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

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"tag":{"type":"keyword"},"n":{"type":"integer"},"o":{"properties":{"t":{"type":"text"}}}}}"#;

fn planned(highlight: &str) -> Result<Option<Plan>, String> {
    let spec = parse(highlight).expect("parses");
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let mut query = BooleanQuery::default();
    query
        .should
        .push(Clause::Term(TermQuery::new("title", "fox")));
    plan(&spec, &mapping, &query).map_err(|e| e.failure.reason)
}

fn names(p: &Plan) -> Vec<(&str, bool)> {
    p.fields
        .iter()
        .map(|f| (f.name.as_str(), f.keyword))
        .collect()
}

#[test]
fn a_plan_names_the_mapped_text_and_keyword_fields() {
    let p = planned(r#"{"fields":{"title":{},"nothing":{},"tag":{}}}"#)
        .expect("plans")
        .expect("something");
    assert_eq!(names(&p), [("title", false), ("tag", true)]);
    let p = planned(r#"{"fields":{"*":{}}}"#)
        .expect("plans")
        .expect("some");
    assert_eq!(names(&p), [("o.t", false), ("tag", true), ("title", false)]);
    let p = planned(r#"{"fields":{"o.*":{}}}"#)
        .expect("plans")
        .expect("some");
    assert_eq!(names(&p), [("o.t", false)]);
    assert!(
        planned(r#"{"fields":{"nothing":{}}}"#)
            .expect("plans")
            .is_none()
    );
    assert!(planned(r#"{"fields":{"n*":{}}}"#).expect("plans").is_none());
    assert!(planned("{}").expect("plans").is_none());
}

#[test]
fn a_field_named_twice_keeps_its_last_options() {
    let p = planned(r#"{"fields":{"title":{"fragment_size":5},"ti*":{"fragment_size":6}}}"#)
        .expect("plans")
        .expect("some");
    assert_eq!(names(&p), [("title", false)]);
    assert_eq!(p.fields[0].options.fragment_size, 6);
}

#[test]
fn a_named_field_of_another_type_is_refused() {
    assert_eq!(
        planned(r#"{"fields":{"n":{}}}"#).expect_err("refused"),
        "highlighting [n], which is not a text or keyword field, is not supported by this node yet"
    );
}

fn written(highlight: &str, query: &[(&str, &str)], source: &str) -> Result<String, String> {
    let spec = parse(highlight).expect("parses");
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let mut q = BooleanQuery::default();
    for (field, term) in query {
        q.should.push(Clause::Term(TermQuery::new(*field, *term)));
    }
    let Some(p) = plan(&spec, &mapping, &q).map_err(|e| e.failure.reason)? else {
        return Ok(String::new());
    };
    let mut h = Highlighters::new(&p).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    h.write(&mut out, source.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8(out).expect("utf-8"))
}

#[test]
fn a_match_is_marked_in_its_fragment() {
    let src = r#"{"title":"The quick brown fox"}"#;
    assert_eq!(
        written(r#"{"fields":{"title":{}}}"#, &[("title", "fox")], src).expect("writes"),
        r#","highlight":{"title":["The quick brown <em>fox</em>"]}"#
    );
    assert_eq!(
        written(
            r#"{"pre_tags":["<b>"],"post_tags":["</b>"],"fields":{"title":{}}}"#,
            &[("title", "fox")],
            src
        )
        .expect("writes"),
        r#","highlight":{"title":["The quick brown <b>fox</b>"]}"#
    );
    // Nothing matches, or the field is absent: nothing is written.
    assert_eq!(
        written(r#"{"fields":{"title":{}}}"#, &[("title", "cat")], src).expect("w"),
        ""
    );
    assert_eq!(
        written(
            r#"{"fields":{"title":{}}}"#,
            &[("title", "fox")],
            r#"{"other":"fox"}"#
        )
        .expect("w"),
        ""
    );
}

#[test]
fn no_match_size_shows_the_start_of_an_unmatched_field() {
    let src = r#"{"title":"The quick brown fox"}"#;
    assert_eq!(
        written(
            r#"{"fields":{"title":{"no_match_size":9}}}"#,
            &[("title", "cat")],
            src
        )
        .expect("writes"),
        r#","highlight":{"title":["The quick"]}"#
    );
}

#[test]
fn the_whole_value_a_keyword_and_a_html_encoding() {
    let src = r#"{"title":"a <fox> & more","tag":"fox"}"#;
    assert_eq!(
        written(
            r#"{"encoder":"html","fields":{"title":{"number_of_fragments":0}}}"#,
            &[("title", "fox")],
            src
        )
        .expect("writes"),
        r#","highlight":{"title":["a &lt;<em>fox</em>&gt; &amp; more"]}"#
    );
    assert_eq!(
        written(r#"{"fields":{"tag":{}}}"#, &[("tag", "fox")], src).expect("writes"),
        r#","highlight":{"tag":["<em>fox</em>"]}"#
    );
}

#[test]
fn values_of_an_array_a_path_and_numbers_are_read_as_java_prints_them() {
    let src = r#"{"o":[{"t":"one fox"},{"t":"two"}],"title":["a fox","fox b"]}"#;
    assert_eq!(
        written(r#"{"fields":{"o.t":{}}}"#, &[("o.t", "fox")], src).expect("writes"),
        r#","highlight":{"o.t":["one <em>fox</em>"]}"#
    );
    assert_eq!(
        written(
            r#"{"fields":{"title":{"number_of_fragments":0}}}"#,
            &[("title", "fox")],
            src
        )
        .expect("writes"),
        r#","highlight":{"title":["a <em>fox</em>","<em>fox</em> b"]}"#
    );
    let mut values = Vec::new();
    let mut rec = Recorder::new(1 << 12);
    let mut tokens = tokenizer();
    let doc = r#"{"a":[1,2.50,1e2,true,null,{"b":"x"}],"s":"t"}"#;
    tokens.feed(doc.as_bytes(), &mut rec).expect("tokens");
    tokens.finish(&mut rec).expect("tokens");
    let mut p = rec.parser();
    p.next_token();
    let tree = read(&mut p, Kind::StartObject);
    collect(&tree, &["a"], &mut values);
    assert_eq!(values, ["1", "2.5", "100.0", "true"]);
    values.clear();
    collect(&tree, &["a", "b"], &mut values);
    assert_eq!(values, ["x"]);
    values.clear();
    collect(&tree, &["s", "deeper"], &mut values);
    assert!(values.is_empty());
}

#[test]
fn fields_come_out_in_hash_order_and_blank_fragments_are_dropped() {
    let src = r#"{"title":"fox","tag":"fox"}"#;
    let out = written(
        r#"{"require_field_match":false,"fields":{"title":{},"tag":{}}}"#,
        &[("title", "fox"), ("tag", "fox")],
        src,
    )
    .expect("writes");
    let mut order = Vec::new();
    java_hash_order(&["title", "tag"], &mut order);
    let first = if order == [0, 1] { "title" } else { "tag" };
    let (a, b) = if first == "title" {
        ("title", "tag")
    } else {
        ("tag", "title")
    };
    assert_eq!(
        out,
        format!(r#","highlight":{{"{a}":["<em>fox</em>"],"{b}":["<em>fox</em>"]}}"#)
    );
    // A field whose only fragment is blank shows nothing.
    assert_eq!(
        written(
            r#"{"fields":{"title":{"no_match_size":2}}}"#,
            &[("title", "cat")],
            r#"{"title":"     x"}"#
        )
        .expect("writes"),
        ""
    );
}

#[test]
fn score_order_puts_the_best_fragment_first() {
    let src = r#"{"title":"fox. fox fox fox. fox. nothing. fox fox"}"#;
    let by_position = written(
        r#"{"fields":{"title":{"fragment_size":12,"number_of_fragments":3}}}"#,
        &[("title", "fox")],
        src,
    )
    .expect("writes");
    let by_score = written(
        r#"{"order":"score","fields":{"title":{"fragment_size":12,"number_of_fragments":3}}}"#,
        &[("title", "fox")],
        src,
    )
    .expect("writes");
    assert_ne!(by_position, by_score);
    assert!(
        by_score
            .starts_with(r#","highlight":{"title":["<em>fox</em>. <em>fox</em> <em>fox</em>","#),
        "{by_score}"
    );
}

#[test]
fn the_word_boundary_and_sentence_scanners_cut_differently() {
    let src = r#"{"title":"alpha beta gamma delta fox epsilon zeta eta theta"}"#;
    let word = written(
        r#"{"boundary_scanner":"word","fields":{"title":{"fragment_size":12}}}"#,
        &[("title", "fox")],
        src,
    )
    .expect("writes");
    let fixed = written(
        r#"{"fields":{"title":{"fragment_size":0}}}"#,
        &[("title", "fox")],
        src,
    )
    .expect("writes");
    assert!(word.contains("<em>fox</em>") && fixed.contains("<em>fox</em>"));
    assert_ne!(word, fixed);
}

#[test]
fn a_source_that_cannot_be_highlighted_fails() {
    let big = format!(r#"{{"title":"{}"}}"#, "a".repeat(MAX_ANALYZED_OFFSET));
    assert!(
        written(r#"{"fields":{"title":{}}}"#, &[("title", "a")], &big)
            .expect_err("too long")
            .contains("over 1,000,000 bytes")
    );
    assert!(
        written(r#"{"fields":{"title":{}}}"#, &[("title", "a")], "[1]")
            .expect_err("not an object")
            .contains("not a JSON object")
    );
    assert!(
        written(
            r#"{"fields":{"title":{}}}"#,
            &[("title", "a")],
            r#"{"title":"#
        )
        .is_err()
    );
}

#[test]
fn the_scratch_bound_grows_with_the_source_and_the_tags() {
    let mk = |tag: &str| Plan {
        query: Clause::Boolean(Box::default()),
        fields: vec![FieldPlan {
            name: "t".into(),
            keyword: false,
            options: Options {
                pre_tag: tag.into(),
                post_tag: tag.into(),
                fragment_size: 100,
                number_of_fragments: 5,
                no_match_size: 0,
                score_ordered: false,
                html: false,
                require_field_match: true,
                word_boundary: false,
            },
        }],
    };
    let small = scratch_for(10, &mk("a"));
    assert!(small > scratch_for(5, &mk("a")));
    assert!(small > scratch_for(10, &mk("")));
    assert!(scratch_for(10, &mk("aaaa")) > small);
    assert!(scratch_for(10, &mk("a")) >= 10 * 4);
}

#[test]
fn a_source_of_exactly_the_limit_is_highlighted() {
    let head = r#"{"title":"fox "#;
    let tail = r#""}"#;
    let pad = "a".repeat(MAX_ANALYZED_OFFSET - head.len() - tail.len());
    let src = format!("{head}{pad}{tail}");
    assert_eq!(src.len(), MAX_ANALYZED_OFFSET);
    let out = written(r#"{"fields":{"title":{}}}"#, &[("title", "fox")], &src).expect("writes");
    assert!(
        out.starts_with(r#","highlight":{"title":["<em>fox</em> "#),
        "{}",
        &out[..60]
    );
}

#[test]
fn a_field_the_query_does_not_name_still_shows_its_no_match_text() {
    let src = r#"{"title":"The quick brown fox"}"#;
    assert_eq!(
        written(
            r#"{"fields":{"title":{"no_match_size":9}}}"#,
            &[("tag", "x")],
            src
        )
        .expect("writes"),
        r#","highlight":{"title":["The quick"]}"#
    );
    assert_eq!(
        written(r#"{"fields":{"title":{}}}"#, &[("tag", "x")], src).expect("writes"),
        ""
    );
}

#[test]
fn a_search_with_no_hit_does_not_plan_its_highlight() {
    use crate::search::search;
    let shard = shard();
    let run = |query: &str| {
        let body = format!(r#"{{"query":{query},"highlight":{{"fields":{{"n":{{}}}}}}}}"#);
        let source = crate::dsl::parse_chunks([body.as_bytes()]).expect("parses");
        search(
            &shard,
            "ex",
            &source,
            &lucene_analysis::Analyzer::standard(None),
        )
        .map(|h| h.hits.len())
        .map_err(|e| e.failure.reason)
    };
    assert_eq!(run(r#"{"term":{"tag":"no-such-tag"}}"#), Ok(0));
    assert!(
        run(r#"{"match_all":{}}"#)
            .expect_err("refused")
            .contains("not a text or keyword field")
    );
}

#[test]
fn a_field_that_pins_an_analyzer_is_not_highlighted_with_the_built_in_one() {
    let mapping = Mapping::from_json_pinned(
        r#"{"properties":{"title":{"type":"text"},"t":{"type":"text","analyzer":"ext_a"}}}"#,
        &["ext_a".to_owned()],
    )
    .expect("mapping");
    let mut query = BooleanQuery::default();
    query.should.push(Clause::Term(TermQuery::new("t", "fox")));
    let named = parse(r#"{"fields":{"t":{}}}"#).expect("parses");
    let e = plan(&named, &mapping, &query).expect_err("refused");
    assert_eq!(e.status, 400);
    assert!(
        e.failure.reason.contains("pins an analyzer"),
        "{}",
        e.failure.reason
    );
    // A pattern passes the pinned field over.
    let all = parse(r#"{"fields":{"*":{}}}"#).expect("parses");
    let p = plan(&all, &mapping, &query).expect("plans").expect("some");
    assert_eq!(names(&p), [("title", false)]);
}
