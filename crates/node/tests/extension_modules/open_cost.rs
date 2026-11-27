//! docs/specs/extension-analyzer AC-15: a node reads a module once for the
//! indexes and shards that pin it, not once for each open.

use super::*;

// AC-15: none per shard
#[test]
fn a_second_shard_of_a_pinned_index_opens_without_reading_the_module_again() {
    let r = rig(16);
    let hash = put(&r, &module("analyzer", false, None));
    let pins = settings(&[("ext_a", &hash, 10_000_000)]);
    let requests = |r: &Rig| {
        S3Op::ALL
            .iter()
            .map(|op| r.store.meter().count(*op))
            .sum::<u64>()
    };
    let reads = |r: &Rig| r.store.meter().count(S3Op::Get) + r.store.meter().count(S3Op::Head);
    let reads_before = reads(&r);
    let first = r.source.open(&pins, "t").expect("the first shard opens");
    assert_eq!(
        reads(&r) - reads_before,
        1,
        "one request for the module the first shard opens with"
    );
    let after_first = requests(&r);
    assert_eq!(r.store.meter().count(S3Op::List), 0, "no LIST on an open");
    let charged = r.budget.in_use(Pool::Extensions);

    let second = r.source.open(&pins, "t").expect("the second shard opens");
    assert_eq!(
        requests(&r),
        after_first,
        "a node holding the module reads nothing for another shard"
    );
    assert_eq!(
        r.budget.in_use(Pool::Extensions),
        charged,
        "and holds its code once"
    );
    for set in [&first, &second] {
        let (_, analyzer) = set.analyzers().next().expect("one");
        assert_eq!(analyzer.analyze("probe").expect("answers")[0].term, "probe");
    }
    assert_eq!(requests(&r), after_first, "a call adds no request either");

    // The code is released when the last shard that holds it closes.
    drop((first, second));
    assert_eq!(
        r.budget.in_use(Pool::Extensions),
        0,
        "released with the last holder"
    );
}
