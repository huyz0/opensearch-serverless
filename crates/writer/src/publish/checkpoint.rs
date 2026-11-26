//! A publisher's staged commit: turning staging on, and writing the register
//! as a checkpoint (see [`super::staging`]).

use std::time::Duration;

use oss_blob::SwapOutcome;

use super::PublishError;
use super::publisher::{Published, ShardPublisher, expired};
use super::register::Last;
use super::staging::due;

impl ShardPublisher {
    /// Turns commit staging on for the next publishes (`Some`, the checkpoint
    /// age) or off (`None`, the default). What is staged stays until a
    /// publish or a flush writes it.
    pub fn use_staging(&mut self, checkpoint: Option<Duration>) {
        self.staging = checkpoint;
    }

    /// Whether a commit is staged: the register is behind this publisher.
    #[must_use]
    pub fn has_staged(&self) -> bool {
        self.staged.is_some()
    }

    /// How old the oldest staged commit is: the age of the oldest pack it
    /// names, or of the first commit staged; `None` when nothing is staged.
    #[must_use]
    pub fn staged_age(&self) -> Option<Duration> {
        self.staged.as_ref().map(|s| s.oldest.elapsed())
    }

    /// Writes the staged commit when the oldest staged commit has reached the
    /// checkpoint age; `None` when nothing is staged or it is not due yet.
    ///
    /// # Errors
    ///
    /// As [`Self::flush_staged`].
    pub fn flush_due(&mut self) -> Result<Option<Published>, PublishError> {
        let Some(age) = self.staging else {
            return Ok(None);
        };
        match &self.staged {
            Some(staged) if due(staged.oldest.elapsed(), age) => self.flush_staged(),
            _ => Ok(None),
        }
    }

    /// Writes the staged commit to the register, whatever its age: one swap
    /// over the generation this publisher last wrote and no upload.
    ///
    /// # Errors
    ///
    /// [`PublishError::PackExpired`] when the commit is past the pack
    /// deadline (it is dropped, and the next publish uploads its files
    /// again), [`PublishError::Stale`] when a newer term holds the register,
    /// and the store's failures.
    pub fn flush_staged(&mut self) -> Result<Option<Published>, PublishError> {
        let Some(staged) = self.staged.take() else {
            return Ok(None);
        };
        let Some(last) = self.last.clone() else {
            // The register this commit stands on is no longer known: dropped.
            return Err(PublishError::PackExpired);
        };
        if expired(staged.oldest.elapsed(), self.swap_deadline) {
            self.last = None;
            return Err(PublishError::PackExpired);
        }
        match self.register.swap(last.generation, &staged.manifest)? {
            SwapOutcome::Applied(now) => {
                self.last = Some(Last {
                    generation: now,
                    term: staged.term,
                    writer: Some(staged.writer),
                    files: staged.files,
                });
                Ok(Some(Published {
                    manifest: staged.manifest,
                    generation: now,
                    // ALLOC: none: the flush uploads nothing.
                    blobs: Vec::new(),
                    superseded: false,
                    staged: false,
                }))
            }
            SwapOutcome::Conflict(_) => {
                self.last = None;
                // ALLOC: none: the flush uploads nothing, so an empty list.
                self.after_lost_swap(staged.term, &staged.writer, Vec::new())
                    .map(Some)
            }
        }
    }
}
