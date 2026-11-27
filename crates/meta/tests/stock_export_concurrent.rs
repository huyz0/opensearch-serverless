//! docs/specs/stock-snapshot-export AC-6: takes that run at the same time
//! each end in the repository, one winner a generation, none lost.

mod stock_export_support;

use std::sync::{Arc, Barrier};

use oss_blob::ExportStore;
use oss_meta::{ExportTarget, MAX_ATTEMPTS, StockRepo, open_export_base};
use stock_export_support::{PREFIX, Take, first, publish, shards, store};

#[test]
fn takes_run_on_threads_from_the_same_base_all_end_in_the_repository() {
    const TAKES: usize = 4;
    let store = store();
    let barrier = Arc::new(Barrier::new(TAKES));
    let handles: Vec<_> = (0..TAKES)
        .map(|i| {
            let (store, barrier) = (store.clone(), Arc::clone(&barrier));
            std::thread::spawn(move || {
                let export = ExportStore::new(store.clone(), PREFIX);
                let base = open_export_base(&ExportTarget {
                    store: &export,
                    prefix: PREFIX,
                })
                .unwrap();
                let name = format!("s{i}");
                let uuid = format!("U{i}");
                let take = Take {
                    name: &name,
                    uuid: &uuid,
                    shards: shards(&format!("t{i}"), 2),
                    ..first()
                };
                // Every take starts from the same empty repository.
                barrier.wait();
                publish(&store, Some(base), &take)
            })
        })
        .collect();
    let mut generations = Vec::new();
    for h in handles {
        let done = h.join().unwrap().unwrap();
        assert!(done.attempts <= MAX_ATTEMPTS, "{}", done.attempts);
        generations.push(done.generation);
    }
    generations.sort_unstable();
    assert_eq!(generations, [0, 1, 2, 3], "one winner for each generation");
    let repo = StockRepo::new(&*store, PREFIX);
    let data = repo.repository_data(repo.generation().unwrap()).unwrap();
    let mut names: Vec<_> = data.snapshots.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["s0", "s1", "s2", "s3"]);
    assert_eq!(data.indices.len(), 1);
    assert_eq!(data.indices[0].snapshots.len(), TAKES);
}
