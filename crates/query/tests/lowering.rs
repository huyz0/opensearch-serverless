//! AC-3, AC-4, AC-12: each statement lowers to the search body stock's
//! `_explain` gives (provisional: hand-written from the plugin's documented
//! translation, noise keys such as `boost` and `adjust_pure_negative` left out),
//! and what cannot be pushed is refused.

mod common;

use common::lower;
use serde_json::{Value, json};

fn case(sql: &str, expected: Value) {
    let got = lower(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    assert_eq!(got, expected, "{sql}");
}

fn filter(cond: Value) -> Value {
    json!({"bool": {"filter": [cond]}})
}

#[test]
fn select_star_has_no_source_filter_and_the_default_size() {
    case("SELECT * FROM accounts", json!({"from": 0, "size": 200}));
}

#[test]
fn columns_become_the_source_includes() {
    case(
        "SELECT firstname, age FROM accounts",
        json!({"from": 0, "size": 200, "_source": {"includes": ["firstname", "age"], "excludes": []}}),
    );
}

#[test]
fn limit_and_offset_are_size_and_from() {
    case(
        "SELECT * FROM accounts LIMIT 5 OFFSET 10",
        json!({"from": 10, "size": 5}),
    );
    case(
        "SELECT * FROM accounts LIMIT 10, 5",
        json!({"from": 10, "size": 5}),
    );
}

#[test]
fn a_limit_past_the_size_limit_is_clamped_to_it() {
    case(
        "SELECT * FROM accounts LIMIT 5000",
        json!({"from": 0, "size": 200}),
    );
}

#[test]
fn equality_on_a_keyword_is_a_term() {
    case(
        "SELECT * FROM accounts WHERE lastname = 'Bates'",
        json!({"from": 0, "size": 200, "query": filter(json!({"term": {"lastname": "Bates"}}))}),
    );
}

#[test]
fn equality_on_text_with_a_keyword_subfield_uses_the_subfield() {
    case(
        "SELECT * FROM accounts WHERE firstname = 'Amber'",
        json!({"from": 0, "size": 200, "query": filter(json!({"term": {"firstname.keyword": "Amber"}}))}),
    );
}

#[test]
fn numbers_are_terms_and_ranges() {
    case(
        "SELECT * FROM accounts WHERE age = 30",
        json!({"from": 0, "size": 200, "query": filter(json!({"term": {"age": 30}}))}),
    );
    for (op, key) in [("<", "lt"), ("<=", "lte"), (">", "gt"), (">=", "gte")] {
        case(
            &format!("SELECT * FROM accounts WHERE age {op} 30"),
            json!({"from": 0, "size": 200, "query": filter(json!({"range": {"age": {key: 30}}}))}),
        );
    }
    case(
        "SELECT * FROM accounts WHERE balance > 1.5",
        json!({"from": 0, "size": 200, "query": filter(json!({"range": {"balance": {"gt": 1.5}}}))}),
    );
}

#[test]
fn between_in_and_null_tests() {
    case(
        "SELECT * FROM accounts WHERE age BETWEEN 20 AND 30",
        json!({"from": 0, "size": 200, "query": filter(json!({"range": {"age": {"gte": 20, "lte": 30}}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE age NOT BETWEEN 20 AND 30",
        json!({"from": 0, "size": 200, "query": filter(json!({"bool": {"must_not": [{"range": {"age": {"gte": 20, "lte": 30}}}]}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE city IN ('a', 'b')",
        json!({"from": 0, "size": 200, "query": filter(json!({"terms": {"city": ["a", "b"]}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE city NOT IN ('a')",
        json!({"from": 0, "size": 200, "query": filter(json!({"bool": {"must_not": [{"terms": {"city": ["a"]}}]}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE city IS NULL",
        json!({"from": 0, "size": 200, "query": filter(json!({"bool": {"must_not": [{"exists": {"field": "city"}}]}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE city IS NOT NULL",
        json!({"from": 0, "size": 200, "query": filter(json!({"exists": {"field": "city"}}))}),
    );
}

#[test]
fn not_equal_is_a_negated_term_of_a_present_value() {
    case(
        "SELECT * FROM accounts WHERE city <> 'x'",
        json!({"from": 0, "size": 200, "query": filter(json!({"bool": {"filter": [{"exists": {"field": "city"}}], "must_not": [{"term": {"city": "x"}}]}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE city != 'x'",
        json!({"from": 0, "size": 200, "query": filter(json!({"bool": {"filter": [{"exists": {"field": "city"}}], "must_not": [{"term": {"city": "x"}}]}}))}),
    );
}

#[test]
fn like_is_a_case_insensitive_wildcard_on_keywords() {
    case(
        "SELECT * FROM accounts WHERE city LIKE 'Ab%c_'",
        json!({"from": 0, "size": 200, "query": filter(json!({"wildcard": {"city": {"wildcard": "Ab*c?", "case_insensitive": true}}}))}),
    );
}

#[test]
fn and_or_not_build_bool_queries() {
    let a = json!({"term": {"city": "a"}});
    let b = json!({"range": {"age": {"gt": 3}}});
    case(
        "SELECT * FROM accounts WHERE city = 'a' AND age > 3",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"filter": [a.clone(), b.clone()]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE city = 'a' OR age > 3",
        json!({"from": 0, "size": 200, "query": filter(json!({"bool": {"should": [a.clone(), b.clone()], "minimum_should_match": 1}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE NOT city = 'a'",
        json!({"from": 0, "size": 200, "query": filter(json!({"bool": {"must_not": [a.clone()]}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE (city = 'a' OR age > 3) AND active = true",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"filter": [
            {"bool": {"should": [a, b], "minimum_should_match": 1}},
            {"term": {"active": true}}
        ]}})}),
    );
}

#[test]
fn relevance_functions_score_and_sit_in_must() {
    case(
        "SELECT * FROM accounts WHERE match(address, 'main street')",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"must": [{"match": {"address": {"query": "main street"}}}]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE match(address, 'main street', operator='AND', fuzziness='AUTO')",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"must": [{"match": {"address": {"query": "main street", "operator": "AND", "fuzziness": "AUTO"}}}]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE match_phrase(address, 'main street', slop=2)",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"must": [{"match_phrase": {"address": {"query": "main street", "slop": 2}}}]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE match_phrase_prefix(address, 'main st')",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"must": [{"match_phrase_prefix": {"address": {"query": "main st"}}}]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE match_bool_prefix(address, 'main st')",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"must": [{"match_bool_prefix": {"address": {"query": "main st"}}}]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE multi_match(['firstname^2', address], 'amber')",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"must": [{"multi_match": {"query": "amber", "fields": {"firstname": 2.0, "address": 1.0}}}]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE query_string(['address'], 'main AND street')",
        json!({"from": 0, "size": 200, "query": json!({"bool": {"must": [{"query_string": {"query": "main AND street", "fields": {"address": 1.0}}}]}})}),
    );
    case(
        "SELECT * FROM accounts WHERE match(address, 'x') AND age > 3",
        json!({"from": 0, "size": 200, "query": json!({"bool": {
            "must": [{"match": {"address": {"query": "x"}}}],
            "filter": [{"range": {"age": {"gt": 3}}}]
        }})}),
    );
}

#[test]
fn order_by_sorts_keywords_numbers_text_and_score() {
    case(
        "SELECT * FROM accounts ORDER BY age DESC",
        json!({"from": 0, "size": 200, "sort": [{"age": {"order": "desc", "missing": "_last"}}]}),
    );
    case(
        "SELECT * FROM accounts ORDER BY lastname",
        json!({"from": 0, "size": 200, "sort": [{"lastname": {"order": "asc", "missing": "_first"}}]}),
    );
    case(
        "SELECT * FROM accounts ORDER BY firstname ASC, age DESC",
        json!({"from": 0, "size": 200, "sort": [
            {"firstname.keyword": {"order": "asc", "missing": "_first"}},
            {"age": {"order": "desc", "missing": "_last"}}
        ]}),
    );
    case(
        "SELECT firstname, _score FROM accounts WHERE match(address, 'x') ORDER BY _score DESC",
        json!({"from": 0, "size": 200, "track_scores": true,
            "query": {"bool": {"must": [{"match": {"address": {"query": "x"}}}]}},
            "_source": {"includes": ["firstname"], "excludes": []},
            "sort": [{"_score": {"order": "desc"}}]}),
    );
}

#[test]
fn identifiers_may_be_quoted_and_nested() {
    case(
        "SELECT `age` FROM `accounts` WHERE `employer`.`name` = 'x'",
        json!({"from": 0, "size": 200,
            "_source": {"includes": ["age"], "excludes": []},
            "query": filter(json!({"term": {"employer.name": "x"}}))}),
    );
    case(
        "select a.age from accounts a where a.city = 'x'",
        json!({"from": 0, "size": 200,
            "_source": {"includes": ["age"], "excludes": []},
            "query": filter(json!({"term": {"city": "x"}}))}),
    );
    case(
        "SELECT * FROM accounts -- note\n WHERE /* c */ age > 1",
        json!({"from": 0, "size": 200, "query": filter(json!({"range": {"age": {"gt": 1}}}))}),
    );
}

#[test]
fn string_escapes_and_booleans() {
    case(
        "SELECT * FROM accounts WHERE city = 'O''Brien'",
        json!({"from": 0, "size": 200, "query": filter(json!({"term": {"city": "O'Brien"}}))}),
    );
    case(
        "SELECT * FROM accounts WHERE active = false",
        json!({"from": 0, "size": 200, "query": filter(json!({"term": {"active": false}}))}),
    );
}

#[test]
fn what_cannot_be_pushed_is_refused_with_a_400_and_no_body() {
    for sql in [
        "SELECT city, count(*) FROM accounts GROUP BY city",
        "SELECT count(*) FROM accounts",
        "SELECT * FROM accounts GROUP BY city HAVING count(*) > 1",
        "SELECT * FROM a JOIN b ON a.id = b.id",
        "SELECT * FROM a, b",
        "SELECT * FROM accounts WHERE age IN (SELECT age FROM b)",
        "SELECT * FROM a UNION SELECT * FROM b",
        "SELECT upper(city) FROM accounts",
        "SELECT * FROM accounts WHERE upper(city) = 'X'",
        "SELECT * FROM accounts WHERE age + 1 > 3",
        "SELECT DISTINCT city FROM accounts",
        "SELECT * FROM accounts ORDER BY address",
        "DELETE FROM accounts WHERE age > 3",
        "INSERT INTO accounts VALUES (1)",
        "CREATE TABLE t (a int)",
        "SELECT * FROM accounts ORDER BY age + 1",
        "SELECT row_number() OVER (ORDER BY age) FROM accounts",
    ] {
        let err = lower(sql).expect_err(sql);
        assert_eq!(err.status(), 400, "{sql}");
    }
}

#[test]
fn no_corpus_body_holds_a_script_key() {
    fn has_script(v: &Value) -> bool {
        match v {
            Value::Object(m) => {
                m.keys().any(|k| k.contains("script")) || m.values().any(has_script)
            }
            Value::Array(a) => a.iter().any(has_script),
            _ => false,
        }
    }
    for sql in [
        "SELECT * FROM accounts WHERE age BETWEEN 1 AND 3 AND city LIKE 'a%'",
        "SELECT firstname FROM accounts WHERE match(address, 'x') ORDER BY age",
    ] {
        assert!(!has_script(&lower(sql).unwrap()), "{sql}");
    }
}

#[test]
fn the_names_a_statement_reads_are_listed_as_written() {
    let stmt = oss_query::parse("SELECT * FROM logs-*", &oss_query::Limits::default()).unwrap();
    let oss_query::Statement::Select(sel) = stmt else {
        panic!()
    };
    assert_eq!(sel.index_names(), ["logs-*".to_string()]);
    let stmt = oss_query::parse("SELECT * FROM `my-index`", &oss_query::Limits::default()).unwrap();
    let oss_query::Statement::Select(sel) = stmt else {
        panic!()
    };
    assert_eq!(sel.index_names(), ["my-index".to_string()]);
}
