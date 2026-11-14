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
