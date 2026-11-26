//! docs/specs/search-rescore: `rescore`, end to end over the compat blobs.

use crate::support::{counts, request, start};

const FIRST: &str = r#""query":{"constant_score":{"filter":{"match_all":{}},"boost":2}}"#;
const ANIMAL: &str = r#"{"constant_score":{"filter":{"term":{"tag":"animal"}},"boost":5}}"#;
const FOOD: &str = r#"{"constant_score":{"filter":{"term":{"tag":"food"}},"boost":10}}"#;

fn run(node: &crate::support::Node, tail: &str) -> (u16, String) {
    let tail = if tail.is_empty() {
        String::new()
    } else {
        format!(",{tail}")
    };
    let body = format!(r#"{{"size":10,{FIRST}{tail}}}"#);
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

/// The `_id` and `_score` of each hit, in order.
fn scores(body: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find(r#""_id":""#) {
        rest = &rest[i + 7..];
        let id = rest[..rest.find('"').expect("id")].to_owned();
        let j = rest.find(r#""_score":"#).expect("score") + 9;
        let end = rest[j..].find([',', '}']).expect("end") + j;
        out.push((id, rest[j..end].parse().expect("a number")));
    }
    out
}

fn rescore(window: Option<u32>, inner: &str) -> String {
    let window = window.map_or(String::new(), |w| format!(r#""window_size":{w},"#));
    format!(r#""rescore":{{{window}"query":{{{inner}}}}}"#)
}

fn sorted(mut v: Vec<(String, f64)>) -> Vec<(String, f64)> {
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

// AC-1
#[test]
fn a_total_adds_the_rescore_query_to_what_it_matches() {
    let node = start();
    let (status, body) = run(
        &node,
        &rescore(None, &format!(r#""rescore_query":{ANIMAL}"#)),
    );
    assert_eq!(status, 200, "{body}");
    let got = scores(&body);
    let want = [("1", 7.0), ("2", 7.0), ("5", 7.0), ("3", 2.0), ("4", 2.0)]
        .map(|(i, s)| (i.to_owned(), s))
        .to_vec();
    assert_eq!(sorted(got.clone()), sorted(want));
    assert!(got[..3].iter().all(|(_, s)| *s == 7.0), "{body}");
}

// AC-2
#[test]
fn the_weights_and_modes_combine_as_stock() {
    let node = start();
    for (mode, matched) in [
        ("total", 7.0),
        ("multiply", 10.0),
        ("avg", 3.5),
        ("max", 5.0),
        ("min", 2.0),
    ] {
        let inner = format!(r#""rescore_query":{ANIMAL},"score_mode":"{mode}""#);
        let (status, body) = run(&node, &rescore(None, &inner));
        assert_eq!(status, 200, "{body}");
        let got = scores(&body);
        assert!(
            got.iter()
                .filter(|(id, _)| ["1", "2", "5"].contains(&id.as_str()))
                .all(|(_, s)| *s == matched),
            "{mode}: {body}"
        );
    }
    let inner = format!(
        r#""rescore_query":{ANIMAL},"query_weight":0.5,"rescore_query_weight":2,"score_mode":"multiply""#
    );
    let (_, body) = run(&node, &rescore(None, &inner));
    let got = scores(&body);
    assert_eq!(got[0].1, 10.0, "{body}");
    assert!(got[3..].iter().all(|(_, s)| *s == 1.0), "{body}");
}

// AC-3
#[test]
fn the_window_bounds_what_is_rescored() {
    let node = start();
    let inner = format!(r#""rescore_query":{ANIMAL},"query_weight":3"#);
    let (status, body) = run(&node, &rescore(Some(2), &inner));
    assert_eq!(status, 200, "{body}");
    let got = scores(&body);
    assert_eq!(
        got.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
        [11.0, 11.0, 6.0, 6.0, 6.0],
        "{body}"
    );
    let page = format!(r#""from":1,"size":2,{FIRST},{}"#, rescore(Some(2), &inner));
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(format!("{{{page}}}").as_bytes()),
    );
    let got = scores(&r.text());
    assert_eq!(got.iter().map(|(_, s)| *s).collect::<Vec<_>>(), [11.0, 6.0]);
}

// AC-4
#[test]
fn rescorers_run_in_order() {
    let node = start();
    let both = format!(
        r#""rescore":[{{"query":{{"rescore_query":{ANIMAL}}}}},{{"query":{{"rescore_query":{FOOD},"score_mode":"multiply"}}}}]"#
    );
    let (status, body) = run(&node, &both);
    assert_eq!(status, 200, "{body}");
    let got = scores(&body);
    assert_eq!(got[0], ("4".to_owned(), 20.0), "{body}");
    assert_eq!(got[1].1, 7.0, "{body}");
}

// AC-5
#[test]
fn what_stock_refuses_is_refused() {
    let node = start();
    let ok = format!(r#""rescore_query":{ANIMAL}"#);
    for (tail, reason) in [
        (
            format!(r#""sort":["year"],{}"#, rescore(None, &ok)),
            "Cannot use [sort] option in conjunction with [rescore].",
        ),
        (
            rescore(Some(20000), &ok),
            "Rescore window [20000] is too large. It must be less than [10000].",
        ),
        (
            rescore(None, r#""query_weight":2"#),
            "rescore_query cannot be null",
        ),
        (
            rescore(None, &format!(r#"{ok},"score_mode":"sum""#)),
            "illegal score_mode [sum]",
        ),
        (rescore(Some(0), &ok), "[window_size] below 1"),
    ] {
        let (status, body) = run(&node, &tail);
        assert_eq!(status, 400, "{tail}: {body}");
        assert!(body.contains(reason), "{tail}: {body}");
    }
}

// AC-6
#[test]
fn a_rescore_adds_no_warm_store_request() {
    let node = start();
    let tail = rescore(None, &format!(r#""rescore_query":{ANIMAL}"#));
    let cost = |tail: &str| {
        run(&node, tail);
        let before = counts(&node.store);
        run(&node, tail);
        counts(&node.store)
            .iter()
            .zip(before.iter())
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>()
    };
    assert_eq!(cost(&tail), cost(""));
}
