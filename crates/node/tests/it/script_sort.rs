//! docs/specs/script-sort: `_script` sort in the expression language, end to
//! end over the compat blobs.

use crate::support::{Node, counts, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

fn search(node: &Node, index: &str, sort: &str, extra: &str) -> (u16, String) {
    let body = format!(r#"{{"size":10,{extra}"query":{{"match_all":{{}}}},"sort":[{sort}]}}"#);
    let r = request(
        node.addr(),
        "POST",
        &format!("/{index}/_search"),
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn script(order: &str) -> String {
    format!(
        r#"{{"_script":{{"type":"number","order":"{order}","script":{{"lang":"expression","source":"doc['year'].value * k","params":{{"k":2}}}}}}}}"#
    )
}

/// The `_id` of each hit, in order, and each hit's `sort` array.
fn order(body: &str) -> (Vec<String>, Vec<String>) {
    let (mut ids, mut sorts) = (Vec::new(), Vec::new());
    let mut rest = body;
    while let Some(i) = rest.find(r#""_id":""#) {
        rest = &rest[i + 7..];
        ids.push(rest[..rest.find('"').expect("id")].to_owned());
        let j = rest.find(r#""sort":["#).expect("sort") + 8;
        sorts.push(rest[j..j + rest[j..].find(']').expect("end")].to_owned());
    }
    (ids, sorts)
}

// AC-1, AC-2
#[test]
fn an_expression_sorts_like_the_field_it_reads() {
    let node = start();
    for (asc, field) in [("asc", r#"{"year":"asc"}"#), ("desc", r#"{"year":"desc"}"#)] {
        let (status, scripted) = search(&node, "compat-term", &script(asc), "");
        assert_eq!(status, 200, "{scripted}");
        let (status, plain) = search(&node, "compat-term", field, "");
        assert_eq!(status, 200, "{plain}");
        let (got, values) = order(&scripted);
        let (want, years) = order(&plain);
        // Years 1999, 2001, 2002, 2002, 2010 tie on 2002, so compare the
        // values, which the script doubles and writes as a Java double.
        assert_eq!(got.len(), 5, "{scripted}");
        let doubled: Vec<String> = years
            .iter()
            .map(|y| format!("{}.0", y.parse::<i64>().expect("year") * 2))
            .collect();
        assert_eq!(values, doubled, "{asc}: {scripted}");
        let _ = want;
    }
    // A script key beside a field key.
    let both = format!(r#"{},{{"tag":"asc"}}"#, script("asc"));
    let (status, body) = search(&node, "compat-term", &both, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""sort":[3998.0,"animal"]"#), "{body}");
}

// AC-3
#[cfg(feature = "coordinator")]
#[test]
fn a_two_shard_index_sorts_through_the_coordinator() {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let (status, scripted) = search(&node, "two", &script("desc"), "");
    assert_eq!(status, 200, "{scripted}");
    let (_, plain_sorts) = order(&search(&node, "two", r#"{"year":"desc"}"#, "").1);
    let (_, values) = order(&scripted);
    let doubled: Vec<String> = plain_sorts
        .iter()
        .map(|y| format!("{}.0", y.parse::<i64>().expect("year") * 2))
        .collect();
    assert_eq!(values, doubled, "{scripted}");
}

// AC-4
#[test]
fn what_is_not_served_is_refused() {
    let node = start();
    let src = r#""script":{"lang":"expression","source":"doc['year'].value"}"#;
    for (sort, extra, reason) in [
        (
            format!(r#"{{"_script":{{"type":"string",{src}}}}}"#),
            "",
            "type [string]",
        ),
        (
            format!(r#"{{"_script":{{"type":"number","mode":"min",{src}}}}}"#),
            "",
            "[mode]",
        ),
        (
            r#"{"_script":{"type":"number","script":"1"}}"#.to_owned(),
            "",
            "runs only lang [expression]",
        ),
        (
            format!(r#"{{"_script":{{"type":"number",{src}}}}}"#),
            r#""search_after":[2.0],"#,
            "with [search_after]",
        ),
        (
            r#"{"_script":{"type":"number","script":{"lang":"expression","source":"doc['title'].value"}}}"#
                .to_owned(),
            "",
            "not a number or a date field",
        ),
    ] {
        let (status, body) = search(&node, "compat-term", &sort, extra);
        assert_eq!(status, 400, "{sort}: {body}");
        assert!(body.contains(reason), "{sort}: {body}");
    }
}

// AC-5
#[test]
fn a_script_sort_adds_no_store_request() {
    let node = start();
    let cost = |sort: &str| {
        search(&node, "compat-term", sort, "");
        let before = counts(&node.store);
        search(&node, "compat-term", sort, "");
        counts(&node.store)
            .iter()
            .zip(before.iter())
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>()
    };
    assert_eq!(cost(&script("asc")), cost(r#"{"year":"asc"}"#));
}
