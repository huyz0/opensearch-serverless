//! docs/specs/proactive-takeover AC-23: a peer that answers it does not own a
//! shard is not the end of the write: the sender reads the head and applies
//! the group where it names, taking the shard itself if no live node holds it.

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use oss_node::writer::Routes;
use oss_rest::Exception;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use support::*;

/// Routes that name a peer as owner, and have the peer refuse as not owner.
struct Refusing {
    inner: oss_node::descriptor_routes::DescriptorRoutes,
    sent: Mutex<Vec<String>>,
}

impl Routes for Refusing {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        self.inner.index(name)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Node("peer".to_owned())
    }

    fn forward(&self, node: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        self.sent.lock().expect("lock").push(node.to_owned());
        group
            .iter()
            .map(|_| ItemResult::Failed {
                status: 503,
                error: Exception::new(
                    "not_shard_owner_exception",
                    "this node does not own logs[0] [serverless:not_owner]",
                ),
            })
            .collect()
    }
}

#[test]
fn a_peer_that_refuses_as_not_owner_leaves_the_shard_to_this_node_when_no_live_node_holds_it() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let routes = Arc::new(Refusing {
        inner: oss_node::descriptor_routes::DescriptorRoutes::new(
            oss_meta::DescriptorStore::new(c.store.clone(), "", oss_meta::DeleteMode::Tombstone)
                .expect("names"),
            1 << 16,
        ),
        sent: Mutex::new(Vec::new()),
    });
    let kept = Arc::clone(&routes);
    let two = node_routed(&c, "node-2", 2, true, Some(take), move |_| kept);
    let result = write(&two.role, "x");
    assert!(matches!(result, ItemResult::Written { .. }), "{result:?}");
    assert_eq!(*routes.sent.lock().expect("lock"), ["peer"], "asked once");
    assert!(two.role.held("u1", 0), "taken here after the refusal");
}
