//! docs/specs/extension-analyzer AC-18: a module an open index holds.

use super::*;
use oss_mem::Pool;

// AC-18: an open index's module is resident. Another module that cannot be
// charged is refused, the held one stays charged and answers, and neither the
// refusal nor the call goes to the store.
#[test]
fn an_open_indexs_module_stays_resident_under_pool_pressure() {
    let r = rig(16);
    let held = put(&r, &module("analyzer", false, None));
    let other = put(&r, &module("analyzer", false, Some(7)));
    let set = r
        .source
        .open(&settings(&[("ext_a", &held, 10_000_000)]), "t")
        .expect("opens");
    let charged = r.budget.in_use(Pool::Extensions);
    assert!(charged > 0, "the open holds its module's code");

    // Everything but one MiB is taken elsewhere: a second module cannot open.
    let room = r.budget.available(Pool::Extensions);
    let taken = r
        .budget
        .try_reserve(Pool::Extensions, room - (1 << 20))
        .expect("reserved");
    let e = r
        .source
        .open(&settings(&[("ext_b", &other, 1_000)]), "t")
        .map(drop)
        .expect_err("refused");
    assert!(e.contains(oss_node::search_modules::NO_CAPACITY), "{e}");
    assert!(
        r.budget.in_use(Pool::Extensions) >= charged + (room - (1 << 20)),
        "the held module is not evicted to make room"
    );

    drop(taken);
    let requests = |r: &Rig| r.store.meter().count(S3Op::Get) + r.store.meter().count(S3Op::Head);
    let before = requests(&r);
    let (_, analyzer) = set.analyzers().next().expect("one");
    for _ in 0..3 {
        assert_eq!(analyzer.analyze("probe").expect("answers")[0].term, "probe");
    }
    assert_eq!(requests(&r), before, "a call reads nothing from the store");
}
