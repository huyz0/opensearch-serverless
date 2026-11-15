//! `ListObjectsV2` pages over HTTP.

use std::collections::BTreeSet;

use crate::{assert_counts, fixture, put};
use oss_blob::*;

// AC-7
#[test]
fn listing_2500_keys_takes_three_pages_and_visits_each_once() {
    let f = fixture();
    let all: BTreeSet<String> = (0..2500).map(|i| format!("p/{i:05}")).collect();
    for k in &all {
        put(f.server.store(), k, b"x");
    }
    put(f.server.store(), "q/other", b"x");
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = f
            .client
            // LIST: a test of the LIST page itself.
            .list_prefix("p/", after.as_deref(), 5000, &mut |k, size| {
                assert_eq!(size, 1);
                seen.push(k.to_owned());
            })
            .unwrap();
        after = seen.last().cloned();
        if !page.truncated {
            break;
        }
    }
    assert_eq!(seen.len(), 2500);
    assert_eq!(seen.iter().cloned().collect::<BTreeSet<_>>(), all);
    assert!(seen.windows(2).all(|w| w[0] < w[1]));
    assert_counts(f.client.meter(), &[(S3Op::List, 3)]);
}

// AC-7
#[test]
fn a_page_is_the_next_keys_after_the_cursor_with_their_sizes() {
    let f = fixture();
    for (k, n) in [("a/1", 1usize), ("a/2", 22), ("a/3", 333), ("b/1", 4)] {
        put(f.server.store(), k, &vec![0u8; n]);
    }
    let mut seen = Vec::new();
    let page = f
        .client
        // LIST: a test of the LIST page itself.
        .list_prefix("a/", Some("a/1"), 1, &mut |k, s| {
            seen.push((k.to_owned(), s))
        })
        .unwrap();
    assert_eq!(seen, [("a/2".to_owned(), 22)]);
    assert_eq!(
        page,
        ListPage {
            keys: 1,
            truncated: true
        }
    );
    let target = &f.server.seen()[0].target;
    assert!(target.contains("start-after=a%2F1"), "{target}");
    assert!(target.contains("max-keys=1"), "{target}");
}

// AC-7
#[test]
fn keys_with_xml_specials_decode() {
    let f = fixture();
    let keys = ["x/a&b", "x/<tag>", "x/\"q\"'s", "x/é"];
    for k in keys {
        put(f.server.store(), k, b"1");
    }
    let mut seen = BTreeSet::new();
    let page = f
        .client
        // LIST: a test of the LIST page itself.
        .list_prefix("x/", None, 10, &mut |k, _| {
            seen.insert(k.to_owned());
        })
        .unwrap();
    assert!(!page.truncated);
    assert_eq!(seen, keys.iter().map(|k| k.to_string()).collect());
}

#[test]
fn a_list_failing_before_any_key_is_retried() {
    let f = fixture();
    put(f.server.store(), "p/1", b"1");
    f.server.store().fail_next(S3Op::List, 2);
    let mut n = 0;
    f.client
        // LIST: a test of the LIST page itself.
        .list_prefix("p/", None, 10, &mut |_, _| n += 1)
        .unwrap();
    assert_eq!(n, 1);
    assert_counts(f.client.meter(), &[(S3Op::List, 3)]);
    assert_eq!(f.client.meter().errors(), 2);
}
