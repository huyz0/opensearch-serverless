//! What a search asks for the overlay of a shard it serves.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lucene_search::directory_reader::DirectoryReader;
use oss_doc::mapping::Mapping;
use oss_ext::analyzer::Analyze;
use oss_format::WalRecord;

use super::{BuildError, Copies, Overlay, candidate_ids, find_copies, local_checkpoint};
use crate::OpenShard;
use crate::commit_push::Stage;

/// A finished overlay with the commit's copies of the ids it replaces: all
/// a search needs to see the shard with the overlay on top.
#[derive(Debug)]
pub struct Ready {
    overlay: Overlay,
    copies: Copies,
    views: AtomicU64,
}

impl Ready {
    /// `overlay` with `copies`, found in the commit it was built over.
    #[must_use]
    pub fn new(overlay: Overlay, copies: Copies) -> Self {
        Self {
            overlay,
            copies,
            views: AtomicU64::new(0),
        }
    }

    /// The overlay.
    #[must_use]
    pub fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    /// How many views this overlay has been asked for: a search that finds
    /// its shard's view cached asks for none.
    #[must_use]
    pub fn views_made(&self) -> u64 {
        self.views.load(Ordering::Relaxed)
    }

    /// The shard as a search sees it with this overlay on top.
    ///
    /// # Errors
    ///
    /// As [`Overlay::view_of`].
    pub fn view<S: oss_blob::BlobStore + 'static>(
        &self,
        shard: &OpenShard<S>,
    ) -> Result<OpenShard<S>, BuildError> {
        self.views.fetch_add(1, Ordering::Relaxed);
        shard.with_overlay(&self.overlay, &self.copies)
    }
}

/// Builds the overlay of `records`, given in ordinal order, over `commit`:
/// reads the commit's local checkpoint and its copies of the ids above it,
/// and leaves out every record the commit already holds. `mapping_json` is
/// the index's mapping source and `max_bytes` what the caller reserved.
///
/// # Errors
///
/// As [`Overlay::build_over`], and [`BuildError::Engine`] for a mapping that
/// does not parse or a commit that cannot be searched for the ids.
pub fn build_ready(
    commit: &DirectoryReader,
    mapping_json: &str,
    records: &[WalRecord<'_>],
    max_bytes: usize,
) -> Result<Ready, BuildError> {
    build_ready_with(commit, mapping_json, records, max_bytes, &[])
}

/// As [`build_ready`], indexing the fields that pin an analyzer with the
/// module `modules` names.
///
/// # Errors
///
/// As [`build_ready`].
pub fn build_ready_with(
    commit: &DirectoryReader,
    mapping_json: &str,
    records: &[WalRecord<'_>],
    max_bytes: usize,
    modules: &[(String, Arc<dyn Analyze>)],
) -> Result<Ready, BuildError> {
    let checkpoint = local_checkpoint(commit);
    let ids = candidate_ids(records, checkpoint)?;
    let copies = find_copies(commit, &ids)?;
    let text = (!mapping_json.trim().is_empty()).then_some(mapping_json);
    let mapping = if modules.is_empty() {
        Mapping::from_descriptor(text)
    } else {
        Mapping::from_descriptor_pinned(text)
    }
    // ALLOC: one message on a failed build.
    .map_err(|e| BuildError::Engine(format!("mapping: {e:?}")))?;
    let overlay =
        Overlay::build_over_with(&mapping, records, checkpoint, max_bytes, &copies, modules)?;
    Ok(Ready::new(overlay, copies))
}

/// The commit a shard is open on, held by a build that outlives the search
/// which asked for it.
pub trait CommitAccess: Send + Sync {
    /// The commit's reader.
    fn reader(&self) -> &DirectoryReader;

    /// The mapping's JSON source, as the index's descriptor gave it.
    fn mapping_json(&self) -> &str;

    /// The name of the index the commit belongs to.
    fn index_name(&self) -> &str;

    /// The analyzers the index's pins loaded, by pin name; none for an
    /// index that pins nothing.
    fn modules(&self) -> Vec<(String, Arc<dyn Analyze>)> {
        // ALLOC: empty; no allocation.
        Vec::new()
    }
}

/// Why a shard's overlay could not be built: a pinned analyzer's module call
/// failed, which a search answers retryably instead of serving without the
/// tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionRefusal {
    /// `extension_busy` (capacity) or another `extension_*` type.
    pub ty: &'static str,
    /// What the module said.
    pub reason: String,
}

impl ExtensionRefusal {
    /// The refusal a failed build is, when a module call failed it.
    #[must_use]
    pub fn of(e: &BuildError) -> Option<Self> {
        match e {
            BuildError::Extension { ty, reason, .. } => Some(Self {
                ty,
                // ALLOC: one message on a failed build.
                reason: reason.clone(),
            }),
            _ => None,
        }
    }
}

/// Where a node's searches get the overlay of a shard. A search never
/// starts a build: the source answers with what is finished, or nothing.
pub trait OverlaySource: Send + Sync {
    /// The finished overlay for shard `shard` of the index `index_uuid`,
    /// built over the commit `commit` (at `stage`), as of `now_millis`.
    fn ready(
        &self,
        index_uuid: &str,
        shard: u32,
        stage: &Stage,
        commit: &Arc<dyn CommitAccess>,
        now_millis: u64,
    ) -> Option<Arc<Ready>>;

    /// Has the overlay of shard `shard` of `index_uuid` cover every record
    /// held for it now, before this returns, so a search that follows
    /// serves them. A source that cannot (no room, a record the engine
    /// refuses) marks the shard so it holds no grant. Nothing by default:
    /// [`Self::ready`] answers with what is finished.
    fn apply_pending(
        &self,
        _index_uuid: &str,
        _shard: u32,
        _stage: &Stage,
        _commit: &Arc<dyn CommitAccess>,
    ) {
    }

    /// The module failure that stopped the latest build of this shard's
    /// overlay, if one did. None by default.
    fn extension_refusal(
        &self,
        _index_uuid: &str,
        _shard: u32,
        _stage: &Stage,
    ) -> Option<ExtensionRefusal> {
        None
    }

    /// The engine open on shard `shard` of `index_uuid` at `stage` let go:
    /// what the source kept for that engine can go too. Nothing by default.
    fn closed(&self, _index_uuid: &str, _shard: u32, _stage: &Stage, _now_millis: u64) {}
}
