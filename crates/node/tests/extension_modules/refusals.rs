//! docs/specs/extension-refusal-metric AC-1: each refused open is counted by
//! why, and an open that succeeds counts nothing.

use super::*;
use oss_node::extension_refusals::totals;

#[test]
fn each_refused_open_counts_once_by_why_and_a_good_open_counts_nothing() {
    let r = rig(16);
    let good = put(&r, &module("analyzer", false, None));
    let (busy0, unavailable0) = totals();

    r.source
        .open(&settings(&[("ext_a", &good, 1_000)]), "t")
        .expect("opens");
    r.source
        .open(br#"{"index.refresh_interval":"1s"}"#, "t")
        .expect("an index with no pins opens");
    assert_eq!(totals(), (busy0, unavailable0), "nothing refused");

    assert!(
        r.source
            .open(&settings(&[("ext_a", HASH_ABSENT, 1_000)]), "t")
            .is_err()
    );
    assert_eq!(totals(), (busy0, unavailable0 + 1), "a missing module");

    // Every byte but one MiB of the extensions pool is held elsewhere.
    let room = r.budget.limit(Pool::Extensions);
    let taken = r
        .budget
        .try_reserve(Pool::Extensions, room - (1 << 20))
        .expect("reserved");
    let other = put(&r, &module("analyzer", false, Some(7)));
    assert!(
        r.source
            .open(&settings(&[("ext_b", &other, 1_000)]), "t")
            .is_err()
    );
    assert_eq!(
        totals(),
        (busy0 + 1, unavailable0 + 1),
        "no room to compile"
    );
    drop(taken);
}
