//! `_search` hit options end to end: `version`, `seq_no_primary_term` and
//! `min_score`, over the recorded fixture indexes.

use crate::support::{request, start};

/// docs/specs/reader-hit-version AC-1, AC-2 and AC-3.
#[test]
fn a_hit_carries_its_version_and_sequence_identity_when_asked() {
    let node = start();
    let ask = |options: &str| {
        let body = format!(
            r#"{{"query":{{"term":{{"title":"brown"}}}},"sort":[{{"_doc":"asc"}}],"size":1{options}}}"#
        );
        let r = request(
            node.addr(),
            "POST",
            "/compat-term/_search",
            Some(body.as_bytes()),
        );
        (r.status, r.text())
    };
    let hit_start = r#""hits":[{"_index":"compat-term","_id":"1","#;
    for (options, carried) in [
        ("", ""),
        (r#","version":false,"seq_no_primary_term":false"#, ""),
        (r#","version":true"#, r#""_version":1,"#),
        (
            r#","seq_no_primary_term":true"#,
            r#""_seq_no":0,"_primary_term":1,"#,
        ),
        (
            r#","version":true,"seq_no_primary_term":true"#,
            r#""_version":1,"_seq_no":0,"_primary_term":1,"#,
        ),
    ] {
        let (status, body) = ask(options);
        assert_eq!(status, 200, "{options}: {body}");
        let want = format!(r#"{hit_start}{carried}"_score":null,"#);
        assert!(body.contains(&want), "{options}: {body}");
    }
    // The values are the doc values a docvalue_fields request reads.
    let (_, both) =
        ask(r#","seq_no_primary_term":true,"docvalue_fields":["_seq_no","_primary_term"]"#);
    assert!(
        both.contains(r#""fields":{"_seq_no":[0],"_primary_term":[1]}"#),
        "{both}"
    );
    // A null is not a boolean, as for `explain`.
    for key in ["version", "seq_no_primary_term"] {
        let (status, body) = ask(&format!(r#","{key}":null"#));
        assert_eq!(status, 400, "{key}: {body}");
    }
}

/// The `_score` values of a response's hits, in order.
fn scores_of(body: &str) -> Vec<String> {
    body.split("\"_score\":")
        .skip(1)
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect()
        })
        .collect()
}

/// docs/specs/reader-min-score AC-1 to AC-4.
#[test]
fn min_score_keeps_and_counts_only_the_matches_scoring_that_much() {
    let node = start();
    let ask = |options: &str| {
        let body = format!(r#"{{"query":{{"match":{{"title":"brown fox"}}}}{options}}}"#);
        let r = request(
            node.addr(),
            "POST",
            "/compat-term/_search",
            Some(body.as_bytes()),
        );
        (r.status, r.text())
    };
    let (status, plain) = ask("");
    assert_eq!(status, 200, "{plain}");
    let scores = scores_of(&plain);
    assert!(
        scores.len() >= 2,
        "the fixture needs two scored hits: {plain}"
    );
    let (best, worst) = (scores[0].clone(), scores[scores.len() - 1].clone());
    assert_ne!(best, worst, "{plain}");
    let kept = |scores: &[String], min: f32| {
        scores
            .iter()
            .filter(|s| s.parse::<f32>().expect("a score") >= min)
            .count()
    };
    let total_of = |body: &str| -> usize {
        let (_, rest) = body.split_once("\"total\":{\"value\":").expect("a total");
        rest.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .expect("a number")
    };
    // Each score of the plain search as the floor keeps what scores at least that.
    for min in &scores {
        let floor: f32 = min.parse().expect("a score");
        let (status, body) = ask(&format!(r#","min_score":{min}"#));
        assert_eq!(status, 200, "{min}: {body}");
        let want = kept(&scores, floor);
        assert_eq!(total_of(&body), want, "{min}: {body}");
        assert_eq!(scores_of(&body).len(), want, "{min}: {body}");
        assert_eq!(scores_of(&body)[0], best, "{min}: {body}");
    }
    // A floor above every score keeps none, and reports no best score.
    let (status, none) = ask(r#","min_score":1000"#);
    assert_eq!(status, 200, "{none}");
    assert_eq!(total_of(&none), 0, "{none}");
    assert!(none.contains(r#""max_score":null,"hits":[]"#), "{none}");
    // A string holding a number is a number, as `floatValue` reads it.
    let (status, text) = ask(r#","min_score":"1000""#);
    assert_eq!((status, total_of(&text)), (200, 0), "{text}");
    // The same floor on a field sort and on a count only.
    let mid: f32 = best.parse::<f32>().expect("a score");
    let (_, sorted) = ask(&format!(
        r#","sort":[{{"_doc":"asc"}},"_score"],"min_score":{mid}"#
    ));
    assert_eq!(total_of(&sorted), kept(&scores, mid), "{sorted}");
    let (_, counted) = ask(&format!(r#","size":0,"min_score":{mid}"#));
    assert_eq!(total_of(&counted), kept(&scores, mid), "{counted}");
    // Not a number, null, and aggregations beside it are refused.
    for options in [
        r#","min_score":"x""#,
        r#","min_score":null"#,
        r#","min_score":1,"aggs":{"a":{"max":{"field":"year"}}}"#,
    ] {
        let (status, body) = ask(options);
        assert_eq!(status, 400, "{options}: {body}");
    }
}
