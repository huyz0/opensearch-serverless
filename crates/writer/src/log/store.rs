//! Appends, reads and deletes of one node's log, by name.

use std::borrow::Cow;
use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ByteRange, CreateOutcome, SliceSource};
use oss_format::node_log_format::{PREFIX_BYTES, decode, header_end};
use oss_format::{NodeLogEntry, NodeLogFence, NodeLogHeader, Slice, node_log_key};

use super::LogError;

/// How many leading bytes a header read fetches first; a longer header
/// costs a second read.
pub const HEADER_PROBE_BYTES: u64 = 64 * 1024;

/// The default and largest [`NodeLogStore::with_max_read`]: the Java's
/// `int`, the most its reader takes in one read.
pub const MAX_READ_BYTES: u64 = 0x7FFF_FFFF;

/// The buffer a PUT or GET streams through, on the stack.
pub(super) const IO_BUF_BYTES: usize = 16 * 1024;

/// One node's write log.
///
/// Holds no state beyond where the log is: the next ordinal, the open
/// epochs and how far the log may be trimmed belong to the writer that
/// owns it.
#[derive(Clone)]
pub struct NodeLogStore {
    pub(super) store: Arc<dyn BlobStore>,
    base: String,
    node_id: String,
    max_read: u64,
}

impl std::fmt::Debug for NodeLogStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeLogStore")
            .field("base", &self.base)
            .field("node_id", &self.node_id)
            .field("max_read", &self.max_read)
            .finish_non_exhaustive()
    }
}

impl NodeLogStore {
    /// The log of `node_id` under the cluster's `base` (empty, or ending in
    /// `/`).
    ///
    /// # Errors
    ///
    /// [`LogError::Format`] if `base` or `node_id` cannot be part of a key.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: impl Into<String>,
        node_id: impl Into<String>,
    ) -> Result<Self, LogError> {
        let log = Self {
            store,
            base: base.into(),
            node_id: node_id.into(),
            max_read: MAX_READ_BYTES,
        };
        log.key(0, 0)?;
        Ok(log)
    }

    /// Bounds every read at `max_read` bytes, at most [`MAX_READ_BYTES`]: a
    /// header, slice or body the blob says is longer is refused before it
    /// is fetched, so what a blob claims never sizes an allocation past it.
    #[must_use]
    pub fn with_max_read(mut self, max_read: u64) -> Self {
        self.max_read = max_read.min(MAX_READ_BYTES);
        self
    }

    /// The node whose log this is.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// The store the log lives in.
    pub fn store(&self) -> &Arc<dyn BlobStore> {
        &self.store
    }

    /// A slot's key.
    ///
    /// # Errors
    ///
    /// [`LogError::Format`] if the base or node id cannot be part of a key.
    pub fn key(&self, epoch: u64, ordinal: u64) -> Result<String, LogError> {
        Ok(node_log_key(&self.base, &self.node_id, epoch, ordinal)?)
    }

    /// Writes a blob into a slot, refusing if the slot is taken. `ordinal`
    /// must be the one after the writer's last landed blob.
    ///
    /// # Errors
    ///
    /// [`LogError::Taken`] if the slot is taken: by a fence, so the epoch
    /// is closed to this writer. [`LogError::Blob`] if the write failed or
    /// its outcome is unknown; the writer must not use the epoch again.
    pub fn append(&self, epoch: u64, ordinal: u64, blob: &[u8]) -> Result<(), LogError> {
        create(self.store.as_ref(), &self.key(epoch, ordinal)?, blob)
    }

    /// An epoch's first free slot, searching from `from`, every slot below
    /// which is taken: one HEAD when `from` is free, otherwise HEADs
    /// doubling their step to a free slot and halving back to the first.
    ///
    /// # Errors
    ///
    /// [`LogError::Blob`] if a HEAD fails.
    pub fn first_free(&self, epoch: u64, from: u64) -> Result<u64, LogError> {
        if !self.taken(epoch, from)? {
            return Ok(from);
        }
        // Taken slots form a prefix: double until a free slot, then halve.
        let mut taken = from;
        let mut step = 1u64;
        let mut free = loop {
            let probe = taken
                .checked_add(step)
                .ok_or(LogError::OutOfRange("slot"))?;
            if !self.taken(epoch, probe)? {
                break probe;
            }
            taken = probe;
            step = step.checked_mul(2).ok_or(LogError::OutOfRange("slot"))?;
        };
        while free.abs_diff(taken) > 1 {
            let mid = taken.midpoint(free);
            if self.taken(epoch, mid)? {
                taken = mid;
            } else {
                free = mid;
            }
        }
        Ok(free)
    }

    fn taken(&self, epoch: u64, ordinal: u64) -> Result<bool, LogError> {
        Ok(self.store.head(&self.key(epoch, ordinal)?)?.is_some())
    }

    /// What a slot holds: a header of records or a fence, `None` when it is
    /// free. One ranged GET of [`HEADER_PROBE_BYTES`]; a second of the
    /// whole header when it is longer.
    ///
    /// # Errors
    ///
    /// [`LogError::Blob`] if a read fails, [`LogError::Format`] if the blob
    /// is malformed.
    pub fn read_entry(
        &self,
        epoch: u64,
        ordinal: u64,
    ) -> Result<Option<NodeLogEntry<'static>>, LogError> {
        let key = self.key(epoch, ordinal)?;
        let leading = match self.read_range(&key, 0, HEADER_PROBE_BYTES) {
            Err(LogError::Blob(BlobError::NotFound)) => return Ok(None),
            read => read?,
        };
        let end = header_end(&leading)?.ok_or(LogError::Short {
            expected: len64(PREFIX_BYTES),
            actual: len64(leading.len()),
        })?;
        if len64(end) <= len64(leading.len()) {
            return Ok(Some(owned(decode(&leading)?)));
        }
        self.check_read(len64(end))?;
        let header = self.read_range(&key, 0, len64(end))?;
        if header.len() != end {
            return Err(LogError::Short {
                expected: len64(end),
                actual: len64(header.len()),
            });
        }
        Ok(Some(owned(decode(&header)?)))
    }

    /// The header of the last blob of records in `[floor, below)`, skipping
    /// fences: its running index lists every shard the writer had not
    /// published, and where, so one header finds every slice in the epoch.
    ///
    /// # Errors
    ///
    /// As [`NodeLogStore::read_entry`].
    pub fn last_header_below(
        &self,
        epoch: u64,
        below: u64,
        floor: u64,
    ) -> Result<Option<NodeLogHeader<'static>>, LogError> {
        for ordinal in (floor..below).rev() {
            if let Some(NodeLogEntry::Records(header)) = self.read_entry(epoch, ordinal)? {
                return Ok(Some(header));
            }
        }
        Ok(None)
    }

    /// One slice of a blob, one ranged GET: its records, one a line, which
    /// [`oss_format::slice_records`] reads one at a time.
    ///
    /// # Errors
    ///
    /// [`LogError::OutOfRange`] if `slice` is not one of the header's;
    /// [`LogError::Blob`] if the read fails; [`LogError::Short`] if it comes
    /// back short.
    pub fn read_slice(
        &self,
        epoch: u64,
        header: &NodeLogHeader<'_>,
        slice: &Slice,
    ) -> Result<Vec<u8>, LogError> {
        if !header.slices.contains(slice) {
            return Err(LogError::OutOfRange("slice"));
        }
        let start = header
            .body_offset
            .checked_add(slice.offset)
            .ok_or(LogError::OutOfRange("slice"))?;
        self.read_exact(epoch, header.ordinal, start, slice.length.into())
    }

    /// The whole body of a blob of records, every slice of it, in one
    /// ranged GET: for a reader that wants the slices of many shards in the
    /// same blob. [`NodeLogStore::parse_slice`] finds a slice in it.
    ///
    /// # Errors
    ///
    /// As [`NodeLogStore::read_slice`].
    pub fn read_body(&self, epoch: u64, header: &NodeLogHeader<'_>) -> Result<Vec<u8>, LogError> {
        self.read_exact(
            epoch,
            header.ordinal,
            header.body_offset,
            header.body_length(),
        )
    }

    /// One slice's bytes out of a body [`NodeLogStore::read_body`] read.
    ///
    /// # Errors
    ///
    /// [`LogError::OutOfRange`] if the slice runs past the body.
    pub fn parse_slice<'b>(body: &'b [u8], slice: &Slice) -> Result<&'b [u8], LogError> {
        let start = usize::try_from(slice.offset).map_err(|_| LogError::OutOfRange("slice"))?;
        let end = usize::try_from(slice.length)
            .ok()
            .and_then(|l| start.checked_add(l))
            .ok_or(LogError::OutOfRange("slice"))?;
        body.get(start..end).ok_or(LogError::OutOfRange("slice"))
    }

    /// Deletes the blobs in `[from, to)`, fences included; empty slots are
    /// skipped by the store. One unbilled DELETE a slot, no LIST.
    ///
    /// # Errors
    ///
    /// [`LogError::Blob`] if a delete fails; the slots before it are gone.
    pub fn delete_range(&self, epoch: u64, from: u64, to: u64) -> Result<(), LogError> {
        for ordinal in from..to {
            self.store.delete(&self.key(epoch, ordinal)?)?;
        }
        Ok(())
    }

    fn read_exact(
        &self,
        epoch: u64,
        ordinal: u64,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, LogError> {
        self.check_read(len)?;
        if len == 0 {
            // ALLOC: none: an empty vector does not allocate.
            return Ok(Vec::new());
        }
        let bytes = self.read_range(&self.key(epoch, ordinal)?, start, len)?;
        if len64(bytes.len()) != len {
            return Err(LogError::Short {
                expected: len,
                actual: len64(bytes.len()),
            });
        }
        Ok(bytes)
    }

    fn check_read(&self, len: u64) -> Result<(), LogError> {
        if len > self.max_read {
            return Err(LogError::OutOfRange("read longer than the store's bound"));
        }
        Ok(())
    }

    /// Up to `len` bytes of `key` from `start`, fewer where it ends.
    fn read_range(&self, key: &str, start: u64, len: u64) -> Result<Vec<u8>, LogError> {
        let cap = usize::try_from(len).map_err(|_| LogError::OutOfRange("read"))?;
        // ALLOC: one read's bytes: a 64 KiB probe, or a header, slice or body
        // checked against `max_read` first (`check_read`); capacity grows
        // with what arrives, never past `len`.
        let mut out = Vec::with_capacity(cap.min(IO_BUF_BYTES));
        self.store.get(
            key,
            ByteRange::From { start, len },
            &mut [0u8; IO_BUF_BYTES],
            &mut |chunk: &[u8]| {
                if out.len().saturating_add(chunk.len()) > cap {
                    return Err(BlobError::Stream("more bytes than the range asked for"));
                }
                out.extend_from_slice(chunk);
                Ok(())
            },
        )?;
        Ok(out)
    }
}

/// A create-if-absent of `blob` at `key`: `Taken` when the slot is taken.
pub(super) fn create(store: &dyn BlobStore, key: &str, blob: &[u8]) -> Result<(), LogError> {
    match store.create_if_absent(key, &mut SliceSource::new(blob), &mut [0u8; IO_BUF_BYTES])? {
        CreateOutcome::Created => Ok(()),
        CreateOutcome::AlreadyExists => Err(LogError::Taken),
    }
}

fn owned(entry: NodeLogEntry<'_>) -> NodeLogEntry<'static> {
    match entry {
        NodeLogEntry::Records(h) => NodeLogEntry::Records(NodeLogHeader {
            node_id: Cow::Owned(h.node_id.into_owned()),
            epoch: h.epoch,
            ordinal: h.ordinal,
            trim_point: h.trim_point,
            body_offset: h.body_offset,
            slices: h.slices,
            index: h.index,
        }),
        NodeLogEntry::Fence(f) => NodeLogEntry::Fence(NodeLogFence {
            fenced_by: Cow::Owned(f.fenced_by.into_owned()),
            term: f.term,
        }),
    }
}

pub(super) fn len64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
