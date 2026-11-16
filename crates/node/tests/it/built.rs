//! A reader node over indexes the engine built (`oss_testkit::index`):
//! more documents and segments than a recorded fixture holds.

use oss_blob::S3Op;
use oss_testkit::index::{Spec, doc_id, doc_source};

use crate::support::{counts, put_built, request, start, start_service, store};

#[test]
fn a_built_index_is_found_with_two_gets_and_served() {
    let node = start();
    let spec = Spec::new("built", 12_000).segments_of(4_000);
    put_built(&node.store, &spec);
    let before = counts(&node.store);
    let r = request(
        node.addr(),
        "POST",
        "/built/_search",
        Some(br#"{"size":2,"sort":[{"n":"desc"}]}"#),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let get = S3Op::ALL
        .iter()
        .position(|op| *op == S3Op::Get)
        .expect("get");
    let gets = counts(&node.store)[get] - before[get];
    assert_eq!(gets, 2, "the descriptor and the commit, then ranged reads");
    let text = r.text();
    assert!(
        text.contains(r#""total":{"value":10000,"relation":"gte"}"#),
        "{text}"
    );
    for i in [11_999, 11_998] {
        let hit = format!(
            r#""_id":"{}","_score":null,"_source":{},"sort":[{i}]"#,
            doc_id(i),
            doc_source(&spec, i)
        );
        assert!(text.contains(&hit), "{hit} in {text}");
    }
}

#[test]
fn an_index_the_open_pool_cannot_hold_is_refused_with_429() {
    let node = start_service(store(), 16 << 10);
    let r = request(node.addr(), "GET", "/compat-term/_search", None);
    assert_eq!(r.status, 429, "{}", r.text());
    let text = r.text();
    assert!(text.contains("circuit_breaking_exception"), "{text}");
    assert!(text.contains("<open_index>"), "{text}");
    assert_eq!(node.service.open_indexes(), 0);
}
