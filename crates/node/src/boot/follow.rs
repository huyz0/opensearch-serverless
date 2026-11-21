//! A node with the tail overlay on follows the cluster register's refresh
//! visibility while it runs.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::BlobStore;
use oss_types::settings::Live;

use super::renew::Renewal;
use super::{BootError, NodeConfig, tail::visibility as node_mode};
use crate::cluster_live::{ClusterFeed, visibility};
use crate::tail_node::VisibilityHandle;
use crate::writer::RefreshVisibility;

/// The thread that reads the cluster register, and what it changes.
pub(super) struct Following {
    handle: VisibilityHandle,
    _thread: Renewal,
}

impl Following {
    /// The mode the writer is in.
    pub(super) fn mode(&self) -> RefreshVisibility {
        self.handle.writer_mode()
    }
}

/// Reads the register once now, so the node starts in the mode the cluster
/// names, and then every `OSS_CLUSTER_SETTINGS_POLL_MS`. `None`, and no
/// request, when the overlay is off (`handle` is `None`): nothing the node
/// runs then reads a dynamic setting.
pub(super) fn start(
    handle: Option<VisibilityHandle>,
    store: Arc<dyn BlobStore>,
    config: &NodeConfig,
) -> Result<Option<Following>, BootError> {
    let Some(handle) = handle else {
        return Ok(None);
    };
    let feed = ClusterFeed::new(store, &config.base, Arc::new(Live::new()))
        .map_err(|e| BootError::Cluster(e.to_string()))?;
    let node = node_mode(config.refresh_visibility);
    let tick = {
        let handle = handle.clone();
        move || {
            if let Ok(true) = feed.read() {
                handle.set(visibility(&*feed.live().current(), node));
            }
        }
    };
    tick();
    let thread = Renewal::spawn_named(
        "cluster-settings",
        Duration::from_millis(config.cluster_settings_poll_millis),
        tick,
    )
    .map_err(|e| BootError::Cluster(format!("cluster settings thread: {e}")))?;
    Ok(Some(Following {
        handle,
        _thread: thread,
    }))
}
