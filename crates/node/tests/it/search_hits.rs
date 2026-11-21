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

/// Each hit's `_id` and `_score` text, in order.
fn id_scores(body: &str) -> Vec<(String, String)> {
    body.split("\"_id\":\"")
        .skip(1)
        .map(|rest| {
            let id: String = rest.chars().take_while(|c| *c != '"').collect();
            let (_, after) = rest.split_once("\"_score\":").expect("a score");
            let score: String = after
                .chars()
                .take_while(|c| c.is_ascii_digit() || c.is_alphabetic() || *c == '.')
                .collect();
            (id, score)
        })
        .collect()
}

/// docs/specs/reader-track-scores AC-1 to AC-4.
#[test]
fn track_scores_scores_the_hits_of_a_field_sort_and_tracks_the_best() {
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
    let (_, plain) = ask("");
    let truth = id_scores(&plain);
    assert!(truth.len() >= 2, "{plain}");
    let best = truth[0].1.clone();
    let sorted = r#","sort":[{"year":"asc"}]"#;
    // Without the key a field sort scores nothing.
    let (status, untracked) = ask(sorted);
    assert_eq!(status, 200, "{untracked}");
    assert!(id_scores(&untracked).iter().all(|(_, s)| s == "null"));
    assert!(untracked.contains(r#""max_score":null"#), "{untracked}");
    // With it each hit has its score of the plain search, and the best is tracked.
    let (status, tracked) = ask(&format!(r#"{sorted},"track_scores":true"#));
    assert_eq!(status, 200, "{tracked}");
    let hits = id_scores(&tracked);
    assert_eq!(hits.len(), truth.len(), "{tracked}");
    for (id, score) in &hits {
        let want = truth.iter().find(|(i, _)| i == id).expect("a plain hit");
        assert_eq!(score, &want.1, "{id}: {tracked}");
    }
    assert!(
        tracked.contains(&format!(r#""max_score":{best},"hits""#)),
        "{tracked}"
    );
    // False is the plain field sort; an unsorted search is unchanged.
    let (_, off) = ask(&format!(r#"{sorted},"track_scores":false"#));
    assert_eq!(id_scores(&off), id_scores(&untracked), "{off}");
    assert!(off.contains(r#""max_score":null"#), "{off}");
    let (_, unsorted) = ask(r#","track_scores":true"#);
    assert_eq!(id_scores(&unsorted), truth, "{unsorted}");
    // A sort led by the score, a null, and a query this node cannot explain are refused.
    for options in [
        r#","sort":["_score",{"year":"asc"}],"track_scores":true"#.to_owned(),
        format!(r#"{sorted},"track_scores":null"#),
    ] {
        let (status, body) = ask(&options);
        assert_eq!(status, 400, "{options}: {body}");
    }
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"query":{"range":{"year":{"gte":1}}},"sort":[{"year":"asc"}],"track_scores":true}"#),
    );
    assert_eq!(r.status, 400, "{}", r.text());
}

/// docs/specs/reader-terminate-after AC-1 to AC-4.
#[test]
fn terminate_after_collects_the_first_matches_and_says_so() {
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
    let (_, plain) = ask("");
    let truth = id_scores(&plain);
    assert!(truth.len() >= 2, "{plain}");
    assert!(!plain.contains("terminated_early"), "{plain}");
    // One match let through: the search stops early, with one hit and a total of one.
    let (status, cut) = ask(r#","terminate_after":1"#);
    assert_eq!(status, 200, "{cut}");
    assert!(
        cut.contains(r#""timed_out":false,"terminated_early":true,"_shards""#),
        "{cut}"
    );
    assert_eq!(id_scores(&cut).len(), 1, "{cut}");
    assert!(
        cut.contains(r#""total":{"value":1,"relation":"eq"}"#),
        "{cut}"
    );
    // More than there are matches: all of them, and no early stop.
    let (_, all) = ask(r#","terminate_after":1000"#);
    assert!(all.contains(r#""terminated_early":false"#), "{all}");
    assert_eq!(id_scores(&all), truth, "{all}");
    // Zero is off; a negative or null value and a size of 0 are refused.
    let (_, off) = ask(r#","terminate_after":0"#);
    assert!(!off.contains("terminated_early"), "{off}");
    for options in [
        r#","terminate_after":-1"#,
        r#","terminate_after":null"#,
        r#","terminate_after":1,"size":0"#,
    ] {
        let (status, body) = ask(options);
        assert_eq!(status, 400, "{options}: {body}");
    }
}
