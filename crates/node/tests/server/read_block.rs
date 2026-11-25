//! docs/specs/index-blocks-read-metadata: a reader refuses to open an index
//! that sets `index.blocks.read`.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, FsStore, PooledStore};
use oss_format::IndexRecord;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::search::start_reader;
use oss_testkit::TempDir;

// AC-1, AC-2
#[test]
fn a_search_of_an_index_with_a_read_block_is_forbidden_and_lifting_it_opens_it() {
    let root = TempDir::new("read-block");
    let blobs = Arc::new(FsStore::new(root.path()).expect("store")) as Arc<dyn BlobStore>;
    let descriptors = DescriptorStore::new(blobs, "", DeleteMode::Conditional).expect("names");
    let mut buf = vec![0u8; 1 << 16];
    let mut blocked = IndexRecord::new("shut", "u1", 1);
    blocked.settings = br#"{"index.blocks.read":"true"}"#;
    descriptors.create(&blocked, 1, &mut buf).expect("created");

    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = BufferPool::new(
        &MemoryBudget::new(&config),
        Pool::Transport,
        SMALL_BUFFER,
        16,
    )
    .expect("fits");
    let reading = Arc::new(PooledStore::new(
        FsStore::new(root.path()).expect("store"),
        buffers,
    ));
    let (reader, _service) = start_reader(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        reading,
        "",
        "reader-1",
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let search = || {
        support::request(
            reader.local_addr(),
            "POST",
            "/shut/_search",
            Some(br#"{"size":0}"#),
        )
    };
    let refused = search();
    assert_eq!(refused.status, 403, "{}", refused.text());
    assert!(
        refused.text().contains("cluster_block_exception")
            && refused
                .text()
                .contains("index [shut] blocked by: [FORBIDDEN/7/index read (api)];"),
        "{}",
        refused.text()
    );

    // Lifting the block: the refused open was not kept, so the next search
    // opens the index afresh and gets past the block (this index has no
    // shard data, so it then fails for that, not for the block).
    let held = descriptors.resolve("shut", &mut buf).expect("reads");
    let oss_meta::Target::Index(record) = held.target else {
        panic!("not an index");
    };
    let mut lifted = record.clone();
    lifted.settings = br#"{"index.blocks.read":"false"}"#;
    let mut out = vec![0u8; 1 << 16];
    assert!(
        descriptors
            .update(&lifted, held.generation, &mut out)
            .expect("swaps")
            .is_some()
    );
    let after = search();
    assert!(!after.text().contains("FORBIDDEN/7"), "{}", after.text());
}
