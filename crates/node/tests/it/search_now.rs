//! docs/specs/search-date-math-now: `now` in a range, from the instant the search arrived.

use crate::support::{request, start};

const PATH: &str = "/compat-term/_search";

fn total(node: &crate::support::Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

fn range(bounds: &str) -> String {
    format!(r#"{{"size":0,"query":{{"range":{{"published":{{{bounds}}}}}}}}}"#)
}

/// The number after `"total":{"value":` in an answer.
fn counted(text: &str) -> u64 {
    let key = r#""total":{"value":"#;
    let from = text.find(key).unwrap_or_else(|| panic!("{text}")) + key.len();
    text[from..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|d| d.parse().ok())
        .expect("a number")
}

#[test]
fn now_in_a_range_is_the_instant_the_search_arrived() {
    let node = start();
    let (status, all) = total(&node, PATH, r#"{"size":0}"#);
    assert_eq!(status, 200, "{all}");
    let every = counted(&all);
    assert!(every > 0, "{all}");
    for (bounds, expected) in [
        (r#""lte":"now""#, every),
        (r#""gt":"now""#, 0),
        (r#""gte":"now-1000y""#, every),
        (r#""gte":"now+1d/d""#, 0),
        (r#""lt":"now+1d","gte":"now-1000y/y""#, every),
    ] {
        let (status, text) = total(&node, PATH, &range(bounds));
        assert_eq!(status, 200, "{bounds}: {text}");
        assert_eq!(counted(&text), expected, "{bounds}: {text}");
    }
}

#[test]
fn now_is_read_in_a_search_over_several_indexes_an_msearch_item_and_a_template() {
    let node = start();
    let body = range(r#""lte":"now""#);
    let (_, one) = total(&node, PATH, &body);
    let (status, many) = total(&node, "/compat-term,compat-bool/_search", &body);
    assert_eq!(status, 200, "{many}");
    assert!(counted(&many) >= counted(&one), "{many}");
    let item = format!("{{\"index\":\"compat-term\"}}\n{body}\n");
    let (status, answer) = total(&node, "/_msearch", &item);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(counted(&answer), counted(&one), "{answer}");
    assert!(!answer.contains("not supported"), "{answer}");
    let template =
        r#"{"source":"{\"size\":0,\"query\":{\"range\":{\"published\":{\"lte\":\"now\"}}}}"}"#;
    let (status, text) = total(&node, "/compat-term/_search/template", template);
    assert_eq!(status, 200, "{text}");
    assert_eq!(counted(&text), counted(&one), "{text}");
}
