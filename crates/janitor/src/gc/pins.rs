//! What else keeps a shard's blobs: frozen views and snapshots.

use oss_format::ShardRef;

use crate::JanitorError;

/// What the pins of one shard hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pinned {
    /// The pins cannot say what they hold: a record that could not be read,
    /// or a capture still reading manifests. Which of the shard's blobs it
    /// will name is unknown, so nothing is deleted.
    Everything,
    /// The blobs, as full keys, a live view or snapshot names.
    Blobs(Vec<String>),
}

/// The pins that keep a shard's blobs beyond its published commit: a point
/// in time paged through, a snapshot (the Java's `pit.pins` and
/// `snapshot.referencedBlobs`).
///
/// The crate ships no implementation that answers "no pins": one would let a
/// sweep delete the files a frozen view is still serving. The point-in-time
/// and snapshot work supply the real ones; a source that cannot tell says
/// [`Pinned::Everything`].
pub trait Pins: Send + Sync {
    /// What the pins hold of `shard`, read once a sweep and before the
    /// listing: a pin made while the sweep runs must be seen first or not
    /// at all.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when the pins cannot be read; nothing is deleted.
    fn pinned(&self, shard: &ShardRef<'_>) -> Result<Pinned, JanitorError>;
}
