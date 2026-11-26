//! Tests of the collapse window; compiled only under `#[cfg(test)]`.

use super::{MAX_CANDIDATES, grow};

#[test]
fn the_window_grows_by_four_to_the_cap_and_no_further() {
    assert_eq!(grow(10, MAX_CANDIDATES), Some(40));
    assert_eq!(grow(30_000, MAX_CANDIDATES), Some(MAX_CANDIDATES));
    assert_eq!(grow(MAX_CANDIDATES, MAX_CANDIDATES), None);
    assert_eq!(grow(usize::MAX, 5), None);
}
