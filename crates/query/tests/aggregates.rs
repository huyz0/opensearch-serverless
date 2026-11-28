//! AC-4, AC-5, AC-6, AC-8: `HAVING`, paging, refusals and no scripts (provisional: the
//! expected values are hand-written; the recorded corpus holds the stock comparison).

use oss_query::{Limits, Statement, parse, plan, rows};
use serde_json::{Value, json};

fn planned(sql: &str) -> Result<oss_query::Plan, oss_query::Error> {
    let Statement::Select(sel) = parse(sql, &Limits::default())? else {
        panic!()
    };
    let mapping = oss_query::Mapping::from_json(&json!({"properties": {
        "city": {"type": "keyword"}, "age": {"type": "long"}, "bio": {"type": "text"}
    }}))
    .unwrap();
    plan(&sel, &mapping, 200)
}

#[test]
fn a_grouped_statement_pages_while_the_answer_has_an_after_key() {
    let p = planned("SELECT city, count(*) FROM t GROUP BY city").unwrap();
    assert!(p.is_paged());
    let full =
        json!({"aggregations": {"composite_buckets": {"buckets": [], "after_key": {"city": "x"}}}});
    let next = p.next_page(&full).unwrap();
    assert_eq!(
        next["aggregations"]["composite_buckets"]["composite"]["after"],
        json!({"city": "x"})
    );
    assert_eq!(next["size"], 0);
    let last = json!({"aggregations": {"composite_buckets": {"buckets": []}}});
    assert!(p.next_page(&last).is_none());
    assert!(!planned("SELECT count(*) FROM t").unwrap().is_paged());
}

#[test]
fn having_keeps_the_buckets_that_pass_and_takes_an_alias() {
    let p = planned("SELECT city, count(*) AS c FROM t GROUP BY city HAVING c >= 2").unwrap();
    let answer = json!({"aggregations": {"composite_buckets": {"buckets": [
        {"key": {"city": "x"}, "doc_count": 3, "count(*)": {"value": 3}},
        {"key": {"city": "y"}, "doc_count": 1, "count(*)": {"value": 1}},
        {"key": {"city": null}, "doc_count": 2, "count(*)": {"value": 2}}
    ]}}});
    assert_eq!(
        rows(&p, &answer).unwrap(),
        vec![vec![json!("x"), json!(3)], vec![Value::Null, json!(2)]]
    );
    let by_call =
        planned("SELECT city, count(*) FROM t GROUP BY city HAVING count(*) > 2").unwrap();
    assert_eq!(
        rows(&by_call, &answer).unwrap(),
        vec![vec![json!("x"), json!(3)]]
    );
}

#[test]
fn what_the_slice_does_not_serve_is_a_400() {
    for sql in [
        "SELECT city, count(*) FROM t GROUP BY city HAVING nope > 1",
        "SELECT sum(city) FROM t",
        "SELECT city, sum(bio) FROM t GROUP BY city",
        "SELECT bio, count(*) FROM t GROUP BY bio",
        "SELECT age, count(*) FROM t GROUP BY city",
        "SELECT * FROM t GROUP BY city",
        "SELECT count(*) FROM t ORDER BY city",
        "SELECT count(distinct age) FROM t",
    ] {
        let err = planned(sql)
            .err()
            .unwrap_or_else(|| panic!("served: {sql}"));
        assert_eq!(err.status(), 400, "{sql}");
    }
}

#[test]
fn no_aggregate_body_holds_a_script() {
    for sql in [
        "SELECT count(*), sum(age), avg(age), min(age), max(age) FROM t WHERE city = 'x'",
        "SELECT DISTINCT city FROM t",
        "SELECT city, avg(age) FROM t GROUP BY city ORDER BY city DESC",
    ] {
        assert!(
            !planned(sql).unwrap().body().to_string().contains("script"),
            "{sql}"
        );
    }
}

#[test]
fn distinct_is_a_composite_with_no_sub_aggregations() {
    let body = planned("SELECT DISTINCT city FROM t")
        .unwrap()
        .body()
        .clone();
    let comp = &body["aggregations"]["composite_buckets"];
    assert!(comp.get("aggregations").is_none());
    assert_eq!(
        comp["composite"]["sources"][0]["city"]["terms"]["missing_bucket"],
        true
    );
}
