//! docs/specs/node-index-cache: the indexes a writer reads, confirmed by the
//! change stripes instead of read again.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, Emulator};
use oss_meta::{ChangeStripes, DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::index_cache::IndexCache;
use oss_node::writer::Routes;

const WINDOW: Duration = Duration::from_secs(5);

struct Rig {
    store: Arc<Emulator>,
    writer: DescriptorStore,
    routes: DescriptorRoutes,
    offset: Arc<Mutex<Duration>>,
}

fn mutator(store: &Arc<Emulator>) -> DescriptorStore {
    let dynstore: Arc<dyn BlobStore> = store.clone();
    DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone)
        .expect("descriptors")
        .record_changes_in(ChangeStripes::new(dynstore, "").expect("stripes"))
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let base = Instant::now();
    let offset = Arc::new(Mutex::new(Duration::ZERO));
    let seen = Arc::clone(&offset);
    let cache = Arc::new(IndexCache::new(
        ChangeStripes::new(Arc::clone(&dynstore), "").expect("stripes"),
        WINDOW,
        Box::new(move || {
            base.checked_add(*seen.lock().expect("clock"))
                .expect("in range")
        }),
    ));
    let routes = DescriptorRoutes::new(
        DescriptorStore::new(dynstore, "", DeleteMode::Tombstone).expect("descriptors"),
        1 << 16,
    )
    .with_cache(cache);
    Rig {
        writer: mutator(&store),
        store,
        routes,
        offset,
    }
}

impl Rig {
    fn create(&self, name: &str, settings: &str) {
        let uuid = format!("u-{name}");
        let mut record = oss_format::IndexRecord::new(name, &uuid, 4);
        record.settings = settings.as_bytes();
        self.writer
            .create(&record, 1, &mut vec![0u8; 1 << 16])
            .expect("created");
    }

    fn change(&self, name: &str, settings: &str) {
        let mut buf = vec![0u8; 1 << 16];
        let generation = self.writer.generation_of(name, &mut buf).expect("exists");
        let uuid = format!("u-{name}");
        let mut record = oss_format::IndexRecord::new(name, &uuid, 4);
        record.settings = settings.as_bytes();
        self.writer
            .update(&record, generation, &mut buf)
            .expect("updated")
            .expect("not moved");
    }

    fn pass(&self, by: Duration) {
        let mut offset = self.offset.lock().expect("clock");
        *offset = offset.saturating_add(by);
    }

    fn requests(&self) -> u64 {
        self.store.meter().total()
    }

    fn partition(&self, name: &str) -> Option<u32> {
        self.routes.index(name).map(|i| i.partition_size)
    }
}

#[test]
fn an_index_read_once_is_served_from_memory_for_a_window() {
    let r = rig();
    r.create("logs", "{}");
    assert_eq!(r.partition("logs"), Some(1));
    let before = r.requests();
    for _ in 0..100 {
        assert_eq!(r.partition("logs"), Some(1));
    }
    assert_eq!(r.requests(), before, "a hundred lookups, no request");
}

#[test]
fn after_a_window_an_unchanged_index_costs_one_stripe_read_however_many_indexes_share_it() {
    let r = rig();
    let names: Vec<String> = (0..40).map(|i| format!("idx-{i}")).collect();
    for name in &names {
        r.create(name, "{}");
        assert_eq!(r.partition(name), Some(1));
    }
    r.pass(WINDOW);
    let before = r.requests();
    for name in &names {
        assert_eq!(r.partition(name), Some(1));
    }
    let spent = r.requests().saturating_sub(before);
    // At most one read of each of the 16 stripes, and no descriptor read.
    assert!(spent <= 16, "{spent} requests for 40 confirmations");
    assert!(spent >= 1);
    let again = r.requests();
    for name in &names {
        assert_eq!(r.partition(name), Some(1));
    }
    assert_eq!(r.requests(), again, "confirmed for another window");
}

#[test]
fn an_update_is_seen_within_a_window_and_not_before() {
    let r = rig();
    r.create("logs", "{}");
    assert_eq!(r.partition("logs"), Some(1));
    r.change("logs", r#"{"index.routing_partition_size":2}"#);
    assert_eq!(r.partition("logs"), Some(1), "inside the window");
    r.pass(WINDOW);
    assert_eq!(r.partition("logs"), Some(2), "past it");
}

#[test]
fn a_delete_is_seen_within_a_window_and_a_recreate_after_it() {
    let r = rig();
    r.create("logs", "{}");
    assert!(r.routes.index("logs").is_some());
    r.writer.delete("logs").expect("deleted");
    r.pass(WINDOW);
    assert!(r.routes.index("logs").is_none());
    r.create("logs", r#"{"index.routing_partition_size":3}"#);
    assert_eq!(r.partition("logs"), Some(3), "a create is seen at once");
}

#[test]
fn an_absent_name_is_never_kept() {
    let r = rig();
    assert!(r.routes.index("later").is_none());
    r.create("later", "{}");
    assert!(r.routes.index("later").is_some(), "no window to wait out");
}

#[test]
fn an_index_another_stripe_holds_is_unaffected_by_a_change_elsewhere() {
    let r = rig();
    let (a, b) = {
        let a = "alpha".to_owned();
        let b = (0..)
            .map(|i| format!("beta-{i}"))
            .find(|n| ChangeStripes::stripe_of(n) != ChangeStripes::stripe_of(&a))
            .expect("another stripe");
        (a, b)
    };
    r.create(&a, "{}");
    r.create(&b, "{}");
    assert_eq!(r.partition(&a), Some(1));
    assert_eq!(r.partition(&b), Some(1));
    r.pass(WINDOW);
    r.change(&b, r#"{"index.routing_partition_size":2}"#);
    // a is confirmed by its own stripe, which the change did not touch.
    assert_eq!(r.partition(&a), Some(1));
    assert_eq!(r.partition(&b), Some(2));
}
