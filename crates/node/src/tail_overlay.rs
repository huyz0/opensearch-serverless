//! Whether a node's searches see the acknowledged writes a writer pushed
//! above the commit they are open on (the shell's
//! `serverless.search.tail_overlay.enabled`).

use std::sync::Arc;

use oss_reader::overlay::OverlaySource;
use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.search.tail_overlay.enabled`: whether a search is served over
/// the commit and an overlay of the writes pushed above it. Default `false`
/// until the overlay's search cost and its effect on write latency are
/// measured with it on.
pub const TAIL_OVERLAY_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.search.tail_overlay.enabled",
    false,
    Scope::Node,
    "Search the acknowledged writes a writer pushed above the open commit, \
     on top of it, before they are published.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[TAIL_OVERLAY_ENABLED.setting()];

/// Installs `source` on `service` when the setting is on (`enabled`); off,
/// a search is answered from the commit alone. Gives whether the source was
/// installed.
pub fn install<S: oss_blob::BlobStore + 'static>(
    service: &crate::search::SearchService<S>,
    enabled: bool,
    source: Arc<dyn OverlaySource>,
) -> bool {
    enabled && service.set_overlay_source(source)
}
