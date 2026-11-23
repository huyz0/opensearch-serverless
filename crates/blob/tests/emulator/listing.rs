//! Bounded, paginated LIST.

use crate::{MIB, emulator, put};
use oss_blob::*;

// AC-7
#[test]
fn listing_pages_through_every_key_once() {
    let e = Emulator::new(64 * MIB, 3);
    for i in 0..2500 {
        put(&e, &format!("idx/{i:05}"), b"x");
    }
    put(&e, "other/1", b"y");
    put(&e, "idx", b"no slash");
    let (mut seen, mut cursor, mut pages) = (Vec::new(), None::<String>, 0);
    loop {
        let mut last = None;
        // LIST: the operation under test.
        let page = e
            .list_prefix("idx/", cursor.as_deref(), 5000, &mut |k, len| {
                assert_eq!(len, 1);
                seen.push(k.to_owned());
                last = Some(k.to_owned());
            })
            .unwrap();
        pages += 1;
        assert!(pages <= 3, "the cursor must move forward");
        assert!(page.keys <= MAX_LIST_KEYS);
        if !page.truncated {
            break;
        }
        cursor = last;
    }
    assert_eq!(pages, 3);
    assert_eq!(e.meter().count(S3Op::List), 3);
    let expected: Vec<String> = (0..2500).map(|i| format!("idx/{i:05}")).collect();
    assert_eq!(seen, expected);
}

proptest::proptest! {
    #[test]
    fn a_page_is_the_next_keys_in_byte_order(
        keys in proptest::collection::btree_set("[a-c]{1,4}", 0..40),
        prefix in "[a-c]{0,2}",
        after in proptest::option::of("[a-c]{0,4}"),
        max in 1usize..10,
    ) {
        let e = emulator();
        for k in &keys {
            put(&e, k, b"");
        }
        let mut got = Vec::new();
        // LIST: the operation under test.
        let page = e.list_prefix(&prefix, after.as_deref(), max, &mut |k, _| got.push(k.to_owned())).unwrap();
        let all: Vec<&String> = keys
            .iter()
            .filter(|k| k.starts_with(&prefix) && after.as_ref().is_none_or(|a| k.as_str() > a.as_str()))
            .collect();
        let want: Vec<String> = all.iter().take(max).map(|k| (*k).clone()).collect();
        proptest::prop_assert_eq!(&got, &want);
        proptest::prop_assert_eq!(page.keys, want.len());
        proptest::prop_assert_eq!(page.truncated, all.len() > max);
    }
}

// AC-7
#[test]
fn children_list_with_one_request_each_and_skip_what_is_under_them() {
    let e = Emulator::new(64 * MIB, 3);
    // Many keys under `a/`, siblings either side of it in byte order.
    for i in 0..50 {
        put(&e, &format!("s/a/{i:03}"), b"x");
    }
    put(&e, "s/a-1/x", b"x");
    put(&e, "s/a0/x", b"x");
    put(&e, "s/b/x", b"x");
    put(&e, "s/leaf", b"x");
    put(&e, "t/other/x", b"x");
    let mut seen = Vec::new();
    // LIST: the operation under test.
    let page = e
        .list_children("s/", None, 10, &mut |c| seen.push(c.to_owned()))
        .unwrap();
    assert_eq!(seen, ["s/a-1/", "s/a/", "s/a0/", "s/b/", "s/leaf"]);
    assert_eq!((page.keys, page.truncated), (5, false));
    // Five children, and one LIST more that found the end.
    assert_eq!(e.meter().count(S3Op::List), 6);
}

// AC-7
#[test]
fn children_resume_after_the_last_one_and_stop_at_the_budget() {
    let e = Emulator::new(64 * MIB, 3);
    for c in ["a", "b", "c", "d"] {
        put(&e, &format!("p/{c}/1"), b"x");
        put(&e, &format!("p/{c}/2"), b"x");
    }
    let mut first = Vec::new();
    // LIST: the operation under test.
    let page = e
        .list_children("p/", None, 2, &mut |c| first.push(c.to_owned()))
        .unwrap();
    assert_eq!(first, ["p/a/", "p/b/"]);
    assert!(page.truncated);
    // Two children, two LISTs: the page stopped at its budget.
    assert_eq!(e.meter().count(S3Op::List), 2);
    let mut rest = Vec::new();
    // LIST: the operation under test.
    let page = e
        .list_children("p/", first.last().map(String::as_str), 10, &mut |c| {
            rest.push(c.to_owned());
        })
        .unwrap();
    assert_eq!(rest, ["p/c/", "p/d/"]);
    assert!(!page.truncated);
    let mut none = Vec::new();
    // LIST: the operation under test.
    e.list_children("zz/", None, 3, &mut |c| none.push(c.to_owned()))
        .unwrap();
    assert!(none.is_empty());
}
