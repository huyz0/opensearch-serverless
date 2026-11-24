//! One node's packs in the object store: written once, by computed name.
//!
//! Ported from `NodePackStore.write` (huyz0/OpenSearch
//! `feature/serverlessshell`). A pack is `packs/<node>/e=<epoch>/<ordinal>`,
//! created only if absent and never changed. It becomes reachable only
//! through a shard's manifest swap, made after the pack landed, so a crash
//! between the two leaves an unreferenced pack for collection and nothing
//! half-written that a reader can see.

use std::sync::Arc;

use oss_blob::{BlobError, BlobResult, BlobStore, ChunkSource, CreateOutcome};
use oss_format::{FileLocation, PlannedBlock, encode_node_pack, node_pack_key};

use super::PackError;

/// The buffer a pack's PUT streams through.
const IO_BUF_BYTES: usize = 64 * 1024;

/// One block to write: a shard's segment blob, streamed from its source.
pub struct Part {
    /// The index's uuid.
    pub index_uuid: String,
    /// The index's name, which collection needs to find the shard's manifest.
    pub index_name: String,
    /// The shard.
    pub shard: u32,
    /// The segment blob's name as the shard would have named it (`_5.seg`).
    pub name: String,
    /// The source of the blob's bytes; its length is the block's.
    pub source: Box<dyn ChunkSource + Send>,
}

impl std::fmt::Debug for Part {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Part")
            .field("index", &self.index_name)
            .field("shard", &self.shard)
            .field("name", &self.name)
            .field("length", &self.source.len())
            .finish()
    }
}

/// Where one blob landed: the pack and the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landed {
    /// The node that wrote the pack.
    pub node: String,
    /// The pack's epoch.
    pub epoch: u64,
    /// The pack's slot in its epoch.
    pub ordinal: u64,
    /// Where the block starts in the pack.
    pub offset: u64,
    /// The block's length.
    pub length: u64,
}

impl Landed {
    /// The location of a range of this block, for a manifest; `None` when
    /// the range is not inside the block.
    pub fn file_at(&self, offset_in_block: u64, length: u64) -> Option<FileLocation<'_>> {
        let end = offset_in_block.checked_add(length)?;
        if end > self.length {
            return None;
        }
        Some(FileLocation::NodePacked {
            node: &self.node,
            epoch: self.epoch,
            ordinal: self.ordinal,
            offset: self.offset.checked_add(offset_in_block)?,
            length,
        })
    }
}

/// A pack as a PUT's body: the header, then each part in order.
struct PackBody {
    header: Vec<u8>,
    parts: Vec<Box<dyn ChunkSource + Send>>,
    len: u64,
    /// Which part is being read: 0 is the header, then the parts.
    part: usize,
    at: usize,
}

impl ChunkSource for PackBody {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let mut n = 0usize;
        // Each pass moves bytes or moves on to the next part, and the parts
        // are finite, so the loop ends.
        while let Some(room) = buf.get_mut(n..).filter(|room| !room.is_empty())
            && self.part <= self.parts.len()
        {
            let taken = if self.part == 0 {
                let rest = self.header.get(self.at..).unwrap_or_default();
                let take = rest.len().min(room.len());
                if let (Some(out), Some(from)) = (room.get_mut(..take), rest.get(..take)) {
                    out.copy_from_slice(from);
                }
                take
            } else {
                match self.parts.get_mut(self.part.saturating_sub(1)) {
                    Some(source) => source.fill(room)?,
                    None => 0,
                }
            };
            if taken == 0 {
                self.part = self.part.saturating_add(1);
                self.at = 0;
                continue;
            }
            n = n.saturating_add(taken);
            self.at = self.at.saturating_add(taken);
        }
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        for part in &mut self.parts {
            part.rewind()?;
        }
        self.part = 0;
        self.at = 0;
        Ok(())
    }
}

/// One node's packs under the cluster's `base`.
pub struct PackStore {
    store: Arc<dyn BlobStore>,
    base: String,
    node: String,
}

impl std::fmt::Debug for PackStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackStore")
            .field("node", &self.node)
            .finish_non_exhaustive()
    }
}

impl PackStore {
    /// The packs of `node` under `base` (empty, or ending in `/`).
    pub fn new(store: Arc<dyn BlobStore>, base: &str, node: &str) -> Self {
        Self {
            store,
            // ALLOC: one entry a part of one publish window, or a key.
            base: base.to_owned(),
            // ALLOC: one entry a part of one publish window, or a key.
            node: node.to_owned(),
        }
    }

    /// The node whose packs these are.
    pub fn node(&self) -> &str {
        &self.node
    }

    /// Writes `parts` as one pack into a slot, refusing if it is taken, and
    /// says where each landed, in order. One PUT whatever the parts.
    ///
    /// # Errors
    ///
    /// [`PackError::Taken`] if the slot exists; [`PackError::Length`] if a
    /// source gives other than its declared length; the store's and the
    /// format's failures. On a failure the pack may or may not exist, and
    /// nothing points at it either way.
    pub fn write(
        &self,
        epoch: u64,
        ordinal: u64,
        parts: Vec<Part>,
    ) -> Result<Vec<Landed>, PackError> {
        // ALLOC: the block plan, one entry a part of one publish window.
        let planned: Vec<PlannedBlock<'_>> = parts
            .iter()
            .map(|p| PlannedBlock {
                index_uuid: &p.index_uuid,
                index_name: &p.index_name,
                shard: p.shard,
                name: &p.name,
                length: p.source.len(),
            })
            // ALLOC: one entry a part of one publish window, or a key.
            .collect();
        let encoded = encode_node_pack(&self.node, epoch, ordinal, &planned)?;
        let landed: Vec<Landed> = planned
            .iter()
            .zip(&encoded.offsets)
            .map(|(p, offset)| Landed {
                // ALLOC: the node id, once a part of one pack.
                node: self.node.clone(),
                epoch,
                ordinal,
                offset: *offset,
                length: p.length,
            })
            // ALLOC: one entry a part of one publish window.
            .collect();
        let key = node_pack_key(&self.base, &self.node, epoch, ordinal)?;
        let len = encoded
            .offsets
            .last()
            .zip(planned.last())
            .map_or(0, |(o, p)| o.saturating_add(p.length));
        let mut body = PackBody {
            header: encoded.bytes,
            // ALLOC: one entry a part of one publish window, or a key.
            parts: parts.into_iter().map(|p| p.source).collect(),
            len,
            part: 0,
            at: 0,
        };
        let mut buf = [0u8; IO_BUF_BYTES];
        match self.store.create_if_absent(&key, &mut body, &mut buf) {
            Ok(CreateOutcome::Created) => Ok(landed),
            Ok(CreateOutcome::AlreadyExists) => Err(PackError::Taken),
            Err(BlobError::InvalidArgument(_)) => {
                Err(PackError::Length("gave other than its declared length"))
            }
            Err(e) => Err(e.into()),
        }
    }
}
