//! Open indexes charged to the `open_indexes` pool, and closed least
//! recently searched first.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::thread;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, S3Op, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::ShardFiles;
use oss_reader::open_cost::open_bytes;
use oss_testkit::index::{Spec, blobs};

use super::{Answer, Held, SearchService, closing_makes_room, never_fits};
use crate::open_indexes::{Found, Lead};

const INDEXES: [&str; 3] = ["idx-a", "idx-b", "idx-c"];

fn store() -> Arc<PooledStore<Emulator>> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for name in INDEXES {
        for (key, bytes) in blobs("", &Spec::new(name, 200)) {
            store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
        }
    }
    Arc::new(store)
}

fn cache() -> Arc<PageCache> {
    Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"))
}

fn budget(open: usize) -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(open)
            .with_pool(Pool::OpenIndexes, open)
            .expect("fits"),
    )
}

/// The charge of each index, as the service will make it.
fn charges(store: &PooledStore<Emulator>, cache: &PageCache) -> Vec<usize> {
    INDEXES
        .iter()
        .map(|name| {
            let (files, meta) = ShardFiles::open_with_meta(store, "", name, 0).expect("locate");
            open_bytes(&files, &meta, cache)
        })
        .collect()
}

fn gets(store: &PooledStore<Emulator>) -> u64 {
    store.store().meter().count(S3Op::Get)
}

/// A service whose pool holds the two largest indexes and not all three.
fn service_for_two() -> (SearchService<Emulator>, MemoryBudget, Vec<usize>) {
    let store = store();
    let cache = cache();
    let charges = charges(&store, &cache);
    let mut sorted = charges.clone();
    sorted.sort_unstable();
    let pool = sorted[1] + sorted[2];
    assert!(pool < charges.iter().sum::<usize>());
    let budget = budget(pool);
    (
        SearchService::new(store, cache, "", "node-1", &budget),
        budget,
        charges,
    )
}

#[test]
fn an_open_index_holds_its_charge_until_the_last_search_ends() {
    let (service, budget, charges) = service_for_two();
    let a = service.shard(INDEXES[0]).ok().expect("opens");
    assert_eq!(budget.in_use(Pool::OpenIndexes), charges[0]);
    let b = service.shard(INDEXES[1]).ok().expect("opens");
    // c does not fit beside a and b, both held: it is refused, and neither
    // is closed.
    assert!(matches!(service.shard(INDEXES[2]), Err(Answer::Rest(_))));
    assert_eq!(service.open_indexes(), 2);
    drop(b);
    assert!(
        service.lock().close_oldest().is_some(),
        "b, no longer held, closes"
    );
    assert!(service.lock().close_oldest().is_none(), "a is held");
    assert_eq!(budget.in_use(Pool::OpenIndexes), charges[0]);
    drop(a);
    assert!(service.lock().close_oldest().is_some());
    assert_eq!(budget.in_use(Pool::OpenIndexes), 0, "every index closed");
}

#[test]
fn an_idle_index_that_cannot_make_room_is_not_closed() {
    let (service, budget, charges) = service_for_two();
    // Two segments: more than b frees beside a held a, within the pool.
    let mid = Spec::new("idx-mid", 200).segments_of(100);
    for (key, bytes) in blobs("", &mid) {
        service
            .store
            .put(&key, &mut SliceSource::new(&bytes))
            .expect("put");
    }
    let (files, meta) =
        ShardFiles::open_with_meta(&service.store, "", "idx-mid", 0).expect("locate");
    let mid_charge = open_bytes(&files, &meta, &service.cache);
    let limit = budget.limit(Pool::OpenIndexes);
    assert!(mid_charge <= limit);
    assert!(
        charges[0] + mid_charge > limit,
        "beside a, closing b cannot make room"
    );
    let a = service.shard(INDEXES[0]).ok().expect("opens");
    drop(service.shard(INDEXES[1]).ok().expect("opens"));
    match service.shard("idx-mid") {
        Err(Answer::Rest(e)) => {
            let mut body = Vec::new();
            e.write_body(&mut body).expect("writes");
            let body = String::from_utf8(body).expect("utf-8");
            assert!(body.contains(r#""durability":"TRANSIENT""#), "{body}");
        }
        _ => panic!("not refused"),
    }
    assert_eq!(service.open_indexes(), 2, "the idle b stays open");
    let before = gets(&service.store);
    drop(service.shard(INDEXES[1]).ok().expect("open"));
    assert_eq!(gets(&service.store), before, "b is still warm");
    drop(a);
    drop(service.shard("idx-mid").ok().expect("now a and b close"));
    assert_eq!(service.open_indexes(), 1);
}

#[test]
fn a_full_pool_closes_the_least_recently_searched_index() {
    let (service, budget, charges) = service_for_two();
    let [a, b, c] = INDEXES;
    for index in [a, b, a] {
        drop(service.shard(index).ok().expect("opens"));
    }
    assert_eq!(budget.in_use(Pool::OpenIndexes), charges[0] + charges[1]);
    drop(service.shard(c).ok().expect("opens"));
    assert_eq!(service.open_indexes(), 2);
    assert_eq!(
        budget.in_use(Pool::OpenIndexes),
        charges[0] + charges[2],
        "b was closed"
    );
    let before = gets(&service.store);
    drop(service.shard(a).ok().expect("still open"));
    assert_eq!(gets(&service.store), before, "a is still open");
    drop(service.shard(b).ok().expect("reopens"));
    assert_eq!(gets(&service.store), before + 2, "b reopens cold");
}

#[test]
fn an_index_larger_than_the_pool_is_refused_and_closes_nothing() {
    let (service, budget, charges) = service_for_two();
    for (key, bytes) in blobs("", &Spec::new("idx-big", 2_000).segments_of(100)) {
        service
            .store
            .put(&key, &mut SliceSource::new(&bytes))
            .expect("put");
    }
    drop(service.shard(INDEXES[0]).ok().expect("opens"));
    match service.shard("idx-big") {
        Err(Answer::Rest(e)) => {
            assert_eq!(e.status(), 429);
            let ex = e.as_exception().expect("an exception");
            assert_eq!(ex.kind(), "circuit_breaking_exception");
            assert!(
                ex.reason()
                    .starts_with("[open_indexes] Data too large, data for [<open_index>]"),
                "{}",
                ex.reason()
            );
            let mut body = Vec::new();
            e.write_body(&mut body).expect("writes");
            let body = String::from_utf8(body).expect("utf-8");
            assert!(body.contains(r#""durability":"PERMANENT""#), "{body}");
        }
        _ => panic!("not refused"),
    }
    assert_eq!(service.open_indexes(), 1, "the open index stays open");
    assert_eq!(budget.in_use(Pool::OpenIndexes), charges[0]);
}

#[test]
fn concurrent_first_searches_open_once_and_close_nothing_else() {
    let (service, budget, charges) = service_for_two();
    let service = Arc::new(service);
    drop(service.shard(INDEXES[0]).ok().expect("opens"));
    let before = gets(&service.store);
    // This thread leads b's first open; the searches started meanwhile
    // must wait for it, not open copies.
    assert!(matches!(service.lock().find(INDEXES[1]), Found::Lead));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let service = Arc::clone(&service);
            thread::spawn(move || service.shard(INDEXES[1]).is_ok())
        })
        .collect();
    // Give them time to queue; any that opened a copy would show as GETs.
    thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(gets(&service.store), before, "nobody opened a copy");
    assert_eq!(budget.in_use(Pool::OpenIndexes), charges[0]);
    let (files, meta) =
        ShardFiles::open_with_meta(&service.store, "", INDEXES[1], 0).expect("locate");
    let charge = service
        .budget
        .try_reserve(Pool::OpenIndexes, open_bytes(&files, &meta, &service.cache))
        .expect("fits");
    let shard = oss_reader::open_located(
        Arc::clone(&service.store),
        Arc::clone(&service.cache),
        "",
        INDEXES[1],
        0,
        files,
        meta,
    )
    .expect("opens");
    drop(Lead::new(&service.open, INDEXES[1]).opened(Arc::new(Held {
        shard,
        view: std::sync::Mutex::default(),
        stage: oss_reader::commit_push::Stage::new(1, None, -1),
        charge,
    })));
    for w in workers {
        assert!(w.join().expect("worker"));
    }
    assert_eq!(gets(&service.store), before + 2, "b opened once");
    assert_eq!(service.open_indexes(), 2);
    assert_eq!(budget.in_use(Pool::OpenIndexes), charges[0] + charges[1]);
    drop(service.shard(INDEXES[0]).ok().expect("open"));
    assert_eq!(gets(&service.store), before + 2, "a is still warm");
}

#[test]
fn closing_makes_room_exactly_when_the_idle_bytes_cover_the_shortfall() {
    let refused = |requested, in_use| oss_mem::Exhausted {
        pool: Pool::OpenIndexes,
        requested,
        in_use,
        limit: 100,
    };
    // 70 in use, 50 wanted: 20 short.
    assert!(!closing_makes_room(&refused(50, 70), 19));
    assert!(closing_makes_room(&refused(50, 70), 20));
    assert!(closing_makes_room(&refused(50, 70), 70));
    // The whole pool fits; one byte more never does, however much is idle.
    assert!(!never_fits(&refused(100, 1)));
    assert!(closing_makes_room(&refused(100, 1), 1));
    assert!(never_fits(&refused(101, 0)));
    assert!(!closing_makes_room(&refused(101, 100), 100));
}

#[test]
fn the_service_clock_counts_milliseconds_from_its_start() {
    let (service, _budget, _charges) = service_for_two();
    let first = service.now_millis();
    thread::sleep(std::time::Duration::from_millis(20));
    let later = service.now_millis();
    assert!(later >= first + 20, "{first} then {later}");
    assert!(later < 60_000, "{later}");
}
