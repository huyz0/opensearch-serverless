//! docs/specs/ism AC-15: the runner's delete is the delete route's delete.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, SliceSource};
use oss_format::IndexRecord;
use oss_meta::{DeleteMode, DescriptorStore, PitClock};
use oss_node::index_delete::{DeleteService, NoSnapshots};

const BUF: usize = 1 << 16;

#[test]
fn the_runner_deletes_an_index_as_the_route_does_and_a_missing_one_is_refused() {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs: Arc<dyn BlobStore> = store.clone();
    let clock: PitClock = Arc::new(|| 1_000_000);
    let service = DeleteService::new(
        &blobs,
        "",
        DeleteMode::Conditional,
        clock,
        Arc::new(NoSnapshots),
        BUF,
        2,
    )
    .expect("service");
    let descriptors =
        DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).expect("store");
    descriptors
        .create(&IndexRecord::new("logs", "u1", 1), 1, &mut vec![0u8; BUF])
        .expect("created");
    let key = "segments/logs#u1#0/seg";
    store
        .put(key, &mut SliceSource::new(b"bytes"), &mut [0u8; 16])
        .expect("put");

    service.delete_index("logs").expect("deleted");
    let mut buf = vec![0u8; BUF];
    assert!(descriptors.get("logs", &mut buf).expect("get").is_none());
    assert!(
        store.head(key).expect("head").is_none(),
        "an unowned shard's bytes go with it"
    );
    let again = service.delete_index("logs").expect_err("gone");
    assert!(again.contains("no such index"), "{again}");
    let many = service.delete_index("a,b").expect_err("one name");
    assert!(many.contains("name one index"), "{many}");
}
