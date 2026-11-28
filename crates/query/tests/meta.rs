//! docs/specs/sql-show-tables AC-2: the prefix a pattern lists loses no name it matches.

use oss_query::{like, prefix_of};

#[test]
fn the_prefix_is_the_text_before_the_first_wildcard() {
    for (pattern, prefix) in [
        ("a%", "a"),
        ("ab_c", "ab"),
        ("%", ""),
        ("%x", ""),
        ("_x", ""),
        ("logs-%", "logs-"),
        ("plain", "plain"),
        ("", ""),
        ("a.b%", "a.b"),
    ] {
        assert_eq!(prefix_of(pattern), prefix, "{pattern}");
    }
}

// A fixed-seed sweep: every name a pattern matches begins with the pattern's prefix.
#[test]
fn no_matching_name_is_lost_to_the_prefix() {
    let alphabet: Vec<char> = "ab-_%.x1".chars().collect();
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % u64::try_from(n).unwrap()).unwrap()
    };
    let mut matched = 0;
    for _ in 0..4_000 {
        let word = |len: usize, next: &mut dyn FnMut(usize) -> usize| -> String {
            (0..len).map(|_| alphabet[next(alphabet.len())]).collect()
        };
        let pattern = word(next(6), &mut next);
        let name: String = word(next(8), &mut next).replace(['%'], "z");
        if like(&pattern, &name) {
            matched += 1;
            assert!(name.starts_with(prefix_of(&pattern)), "{pattern} {name}");
        }
    }
    assert!(matched > 100, "the sweep matched only {matched}");
}
