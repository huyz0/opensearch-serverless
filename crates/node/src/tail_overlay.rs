//! Whether a node's searches see the acknowledged writes a writer pushed
//! above the commit they are open on (the shell's
//! `serverless.search.tail_overlay.enabled`).

use std::sync::Arc;

use oss_reader::overlay::OverlaySource;
use oss_types::settings::{BoolSetting, ChoiceSetting, Scope, Setting};

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

/// `serverless.search.refresh_visibility`: which copies of a shard answer a
/// search only while they hold every write the writer answered as refreshed
/// (the shell's `RefreshVisibility`). Only `everywhere` makes a reader check
/// with the writer before it answers; the writer-side wait of `owner` and
/// `serving` is not wired yet, so both leave a search as it is.
pub const REFRESH_VISIBILITY: ChoiceSetting = ChoiceSetting::new(
    "serverless.search.refresh_visibility",
    "everywhere",
    &["owner", "serving", "everywhere"],
    Scope::Node,
    "Which copies of a shard answer a search only once they hold every write \
     acknowledged as refreshed: owner, serving or everywhere.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[TAIL_OVERLAY_ENABLED.setting(), REFRESH_VISIBILITY.setting()];

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

/// What a node with the tail overlay on holds: the records writers pushed
/// and fetched, the handler for their pushes, and the builder that turns
/// them into overlays.
pub struct Wired {
    /// The records held for the shards this node serves.
    pub held: Arc<crate::tail_held::TailHeld>,
    /// Takes a writer's push into [`Self::held`]; registered on the node's
    /// transport by whoever assembles it.
    pub push: crate::tail_reader_link::TailPushHandler,
    /// The overlay source installed on the search service.
    pub builder: Arc<crate::overlay_builder::OverlayBuilder>,
}

/// Turns the tail overlay on for `service` when the setting is on
/// (`enabled`): a builder charging `budget`'s `open_indexes` pool, asking a
/// shard's writer through `fetcher` as the node `subscriber`. Off, or with a
/// source installed already, nothing is built and the answer is `None`.
pub fn wire<S: oss_blob::BlobStore + 'static>(
    service: &crate::search::SearchService<S>,
    enabled: bool,
    budget: oss_mem::MemoryBudget,
    fetcher: Arc<dyn crate::overlay_builder::TailFetcher>,
    subscriber: String,
    clock: crate::tail_reader_link::Clock,
) -> Option<Wired> {
    // Checked before anything is built: a refused wire starts no worker.
    if !enabled || service.has_overlay_source() {
        return None;
    }
    let held = Arc::new(crate::tail_held::TailHeld::new());
    let builder = crate::overlay_builder::OverlayBuilder::fetching(
        Arc::clone(&held),
        budget,
        fetcher,
        subscriber,
        Arc::clone(&clock),
    );
    if !install(
        service,
        true,
        Arc::clone(&builder) as Arc<dyn OverlaySource>,
    ) {
        return None;
    }
    let push = crate::tail_reader_link::TailPushHandler::new(Arc::clone(&held), clock);
    Some(Wired {
        held,
        push,
        builder,
    })
}
