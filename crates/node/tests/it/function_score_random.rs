//! docs/specs/function-score-random: `random_score` and `min_score`, end to
//! end over the compat blobs.

use crate::support::{counts, request, start};

fn search_with(node: &crate::support::Node, tail: &str) -> (u16, String) {
    let body = format!(r#"{{"size":50,"query":{{"function_score":{{{tail}}}}}}}"#);
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn random(node: &crate::support::Node, function: &str) -> (u16, String) {
    search_with(
        node,
        &format!(r#""query":{{"match_all":{{}}}},"boost_mode":"replace",{function}"#),
    )
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

fn by_id(mut v: Vec<(String, f64)>) -> Vec<(String, f64)> {
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

// AC-1
#[test]
fn a_random_score_without_a_seed_is_in_the_unit_interval() {
    let node = start();
    let (status, body) = random(&node, r#""random_score":{}"#);
    assert_eq!(status, 200, "{body}");
    let got = scores(&body);
    assert!(got.len() > 2, "{body}");
    assert!(got.iter().all(|(_, s)| (0.0..1.0).contains(s)), "{body}");
    assert!(got.windows(2).all(|w| w[0].1 >= w[1].1), "{body}");
}

// AC-2
#[test]
fn a_seed_and_a_field_give_the_same_scores_every_time() {
    let node = start();
    let f = r#""random_score":{"seed":42,"field":"year"}"#;
    let (_, first) = random(&node, f);
    let (_, second) = random(&node, f);
    assert_eq!(scores(&first), scores(&second));
    let (_, other) = random(&node, r#""random_score":{"seed":43,"field":"year"}"#);
    assert_ne!(by_id(scores(&first)), by_id(scores(&other)));
    let (_, text) = random(&node, r#""random_score":{"seed":"42","field":"year"}"#);
    assert_ne!(by_id(scores(&first)), by_id(scores(&text)));
}

// AC-3
#[test]
fn the_score_follows_the_field_it_hashes() {
    let node = start();
    let mut seen = Vec::new();
    for f in [
        r#""random_score":{"seed":7}"#,
        r#""random_score":{"seed":7,"field":"_id"}"#,
        r#""random_score":{"seed":7,"field":"tag"}"#,
        r#""random_score":{"seed":7,"field":"year"}"#,
        r#""random_score":{"seed":7,"field":"published"}"#,
        r#""random_score":{"seed":7,"field":"year","_name":"x"}"#,
    ] {
        let (status, body) = random(&node, f);
        if f.contains("_name") {
            assert_eq!(status, 400, "{body}");
            assert!(body.contains("[function_score] [_name]"), "{body}");
            continue;
        }
        assert_eq!(status, 200, "{f}: {body}");
        seen.push(by_id(scores(&body)));
    }
    assert_eq!(seen[0], seen[1], "no field is _id");
    assert_ne!(seen[1], seen[2]);
    assert_ne!(seen[1], seen[3]);
    assert_ne!(seen[3], seen[4]);
}

// AC-4
#[test]
fn a_seed_that_is_not_stocks_is_refused() {
    let node = start();
    for (function, reason) in [
        (
            r#""random_score":{"seed":1.5}"#,
            "random_score seed must be an int, long or string, not 'VALUE_NUMBER'",
        ),
        (
            r#""random_score":{"seed":true}"#,
            "random_score seed must be an int/long or string, not 'VALUE_BOOLEAN'",
        ),
        (
            r#""random_score":{"sede":1}"#,
            "random_score query does not support [sede]",
        ),
        (
            r#""random_score":{"seed":1,"field":"nope"}"#,
            "Field [nope] is not mapped on [[compat-term/",
        ),
        (
            r#""random_score":{"seed":1,"field":"title"}"#,
            "Text fields are not optimised",
        ),
    ] {
        let (status, body) = random(&node, function);
        assert_eq!(status, 400, "{function}: {body}");
        assert!(body.contains(reason), "{function}: {body}");
    }
    let (status, body) = random(
        &node,
        r#""random_score":{"seed":3000000000,"field":"year"}"#,
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = random(&node, r#""random_score":{"seed":"x","field":"tag"}"#);
    assert_eq!(status, 200, "{body}");
}

// AC-5
#[test]
fn min_score_keeps_what_scores_at_least_it() {
    let node = start();
    let f = r#""random_score":{"seed":5,"field":"year"}"#;
    let (_, all) = random(&node, f);
    let all = scores(&all);
    let mut sorted: Vec<f64> = all.iter().map(|(_, s)| *s).collect();
    sorted.sort_by(f64::total_cmp);
    let cut = sorted[sorted.len() / 2];
    let (status, kept) = random(&node, &format!(r#"{f},"min_score":{cut}"#));
    assert_eq!(status, 200, "{kept}");
    let want: Vec<_> = by_id(all.into_iter().filter(|(_, s)| *s >= cut).collect());
    assert_eq!(by_id(scores(&kept)), want);

    let (status, kept) = search_with(&node, r#""query":{"match_all":{}},"min_score":2"#);
    assert_eq!(status, 200, "{kept}");
    assert!(scores(&kept).is_empty(), "{kept}");
    let (status, kept) = search_with(&node, r#""query":{"match_all":{}},"min_score":1"#);
    assert_eq!(status, 200, "{kept}");
    assert!(!scores(&kept).is_empty(), "{kept}");
    let (status, kept) = search_with(
        &node,
        r#""query":{"match_all":{}},"weight":0.5,"min_score":1"#,
    );
    assert_eq!(status, 200, "{kept}");
    assert!(scores(&kept).is_empty(), "{kept}");
}

// AC-6
#[test]
fn a_random_score_adds_no_store_request() {
    let node = start();
    let plain = r#""field_value_factor":{"field":"year"}"#;
    random(&node, plain);
    let cost = |function: &str| {
        let before = counts(&node.store);
        random(&node, function);
        counts(&node.store)
            .iter()
            .zip(before.iter())
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>()
    };
    let base = cost(plain);
    assert_eq!(cost(r#""random_score":{"seed":1,"field":"year"}"#), base);
    assert_eq!(cost(r#""random_score":{}"#), base);
    assert_eq!(cost(r#""random_score":{},"min_score":0.5"#), base);
}
