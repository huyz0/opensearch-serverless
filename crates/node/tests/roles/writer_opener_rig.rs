//! The opener rig's parts: an opener that counts what it is asked, and
//! routes that forward and say where.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::{HeadOpener, OpenRefused, Routes, ShardOpener, WriterRole};
use oss_rest::Exception;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::index::{Outcome, Written};

/// An opener that counts what it is asked.
pub(super) struct Counting {
    pub(super) inner: HeadOpener,
    pub(super) calls: Arc<AtomicUsize>,
}

impl ShardOpener for Counting {
    fn open(&self, role: &WriterRole, index: &IndexInfo, shard: u32) -> Result<(), OpenRefused> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.open(role, index, shard)
    }
}

/// Routes that place every shard here and answer a forward as `answer` says,
/// counting the forwards and naming where each went.
pub(super) struct Forwarding {
    pub(super) inner: DescriptorRoutes,
    pub(super) sent: Mutex<Vec<String>>,
    pub(super) fail: AtomicBool,
}

impl Routes for Forwarding {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        self.inner.index(name)
    }

    fn owner(&self, index: &IndexInfo, shard: u32) -> Owner {
        self.inner.owner(index, shard)
    }

    fn forward(&self, node: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        self.sent.lock().expect("lock").push(node.to_owned());
        group
            .iter()
            .map(|_| {
                if self.fail.load(Ordering::SeqCst) {
                    ItemResult::Failed {
                        status: 503,
                        error: Exception::new("unavailable_shards_exception", "the holder is gone"),
                    }
                } else {
                    ItemResult::Written {
                        written: Written {
                            outcome: Outcome::Created,
                            version: 1,
                            seq_no: 0,
                            primary_term: 1,
                        },
                        forced_refresh: false,
                    }
                }
            })
            .collect()
    }
}
