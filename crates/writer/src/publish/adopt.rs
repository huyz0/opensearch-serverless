//! A publisher that starts from a commit adopted from a predecessor's log
//! (docs/specs/open-from-logged-commit).

use std::time::Instant;

use super::PublishError;
use super::publisher::ShardPublisher;
use super::register::last_of;
use oss_format::CommitManifest;

impl ShardPublisher {
    /// Makes `manifest`, read at register `generation`, the commit this
    /// publisher builds on: its next publish uploads only what the manifest
    /// does not name and swaps over `generation`. That publish is the only
    /// one that may use it: a lost swap is [`PublishError::AdoptionLost`],
    /// not retried, and a swap after the oldest pack's `packed_at` is past the
    /// swap deadline is [`PublishError::PackExpired`]. Staged sequence
    /// numbers continue above the manifest's.
    ///
    /// # Errors
    ///
    /// [`PublishError::Format`] or [`PublishError::Unusable`] for a manifest
    /// this writer cannot build on; nothing is adopted then.
    pub fn adopt_commit(
        &mut self,
        manifest: &[u8],
        generation: u64,
        packed_at: Instant,
    ) -> Result<(), PublishError> {
        let last = last_of(manifest, generation)?;
        let parsed = CommitManifest::parse(manifest)?;
        self.sequence = self.sequence.max(parsed.stage_sequence);
        self.last = Some(last);
        self.adopted = Some(packed_at);
        Ok(())
    }

    /// Whether a commit adopted from a predecessor's log waits for this
    /// publisher's next publish.
    #[must_use]
    pub fn has_adopted(&self) -> bool {
        self.adopted.is_some()
    }
}
