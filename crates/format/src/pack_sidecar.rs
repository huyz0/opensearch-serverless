//! The collector's sidecar register at `packs/<node>/collected`: per live
//! epoch of a node, a bitmap of the ordinals already deleted or never
//! written, and the epoch's `high`, so a restarted collector re-reads only
//! the headers of ordinals not marked (the live packs), never every ordinal
//! of every epoch.
//!
//! Rust only: the Java's version-1 ledger beside it is untouched and the
//! Java does not read this. A version byte (1), an epoch count (u32), then
//! per epoch the epoch (u64), its `high` (u64) and a bitmap of
//! [`ORDINALS_PER_EPOCH`] bits, all big-endian: 144 bytes an epoch.

use crate::error::{FormatError, corrupt};

const WHAT: &str = "node pack sidecar";
const VERSION: u8 = 1;
const PREFIX_BYTES: usize = 5;
const BITMAP_BYTES: usize = 128;

/// How many ordinals an epoch holds before the writer rotates to a new one.
pub const ORDINALS_PER_EPOCH: u64 = 1024;

/// Bytes an epoch takes in the register: the epoch, its `high`, the bitmap.
pub const SIDECAR_EPOCH_BYTES: usize = 16 + BITMAP_BYTES;

/// The most epochs a sidecar holds, so the register stays under the 256 KiB
/// a register read keeps.
pub const MAX_SIDECAR_EPOCHS: usize = ((256 << 10) - PREFIX_BYTES) / SIDECAR_EPOCH_BYTES;

/// One epoch's collection progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidecarEpoch {
    /// The epoch.
    pub epoch: u64,
    /// One past the highest ordinal reserved in it.
    pub high: u64,
    collected: [u8; BITMAP_BYTES],
}

impl SidecarEpoch {
    /// An epoch with nothing collected yet.
    pub const fn new(epoch: u64, high: u64) -> Self {
        Self {
            epoch,
            high,
            collected: [0; BITMAP_BYTES],
        }
    }

    /// Marks `ordinal` collected; one past the epoch is ignored.
    pub fn mark(&mut self, ordinal: u64) {
        if let Some((byte, bit)) = locate(ordinal)
            && let Some(b) = self.collected.get_mut(byte)
        {
            *b |= bit;
        }
    }

    /// Whether `ordinal` is marked collected.
    pub fn is_marked(&self, ordinal: u64) -> bool {
        locate(ordinal)
            .and_then(|(byte, bit)| self.collected.get(byte).map(|b| b & bit != 0))
            .unwrap_or(false)
    }

    /// Whether every ordinal of the epoch is marked.
    pub fn is_full(&self) -> bool {
        self.collected.iter().all(|b| *b == 0xFF)
    }

    /// The ordinals below `high` not marked: the packs that may still exist.
    pub fn unmarked(&self) -> impl Iterator<Item = u64> + '_ {
        (0..self.high.min(ORDINALS_PER_EPOCH)).filter(|o| !self.is_marked(*o))
    }
}

fn locate(ordinal: u64) -> Option<(usize, u8)> {
    if ordinal >= ORDINALS_PER_EPOCH {
        return None;
    }
    let byte = usize::try_from(ordinal / 8).ok()?;
    Some((byte, 1u8 << (ordinal % 8)))
}

/// A parsed sidecar: the live epochs, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSidecar {
    epochs: Vec<SidecarEpoch>,
}

impl PackSidecar {
    /// A sidecar of `epochs`, oldest first.
    pub const fn new(epochs: Vec<SidecarEpoch>) -> Self {
        Self { epochs }
    }

    /// The epochs, oldest first.
    pub fn epochs(&self) -> &[SidecarEpoch] {
        &self.epochs
    }

    /// The epochs, to mark ordinals in.
    pub fn epochs_mut(&mut self) -> &mut Vec<SidecarEpoch> {
        &mut self.epochs
    }

    /// Drops every full epoch but the newest, which is never dropped.
    pub fn drop_full(&mut self) {
        let newest = self.epochs.len().saturating_sub(1);
        let mut index = 0usize;
        self.epochs.retain(|e| {
            let keep = index == newest || !e.is_full();
            index = index.saturating_add(1);
            keep
        });
    }

    /// Parses a sidecar register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a short or over-long value or one past
    /// the epoch cap, [`FormatError::Unsupported`] for another version.
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let Some((&version, rest)) = bytes.split_first() else {
            return Err(corrupt(WHAT, "empty"));
        };
        if version != VERSION {
            return Err(FormatError::Unsupported {
                what: WHAT,
                version: version.into(),
            });
        }
        let (count, body) = rest
            .split_first_chunk::<4>()
            .ok_or(corrupt(WHAT, "no epoch count"))?;
        let count = usize::try_from(u32::from_be_bytes(*count))
            .map_err(|_| corrupt(WHAT, "an epoch count too large"))?;
        if count > MAX_SIDECAR_EPOCHS {
            return Err(corrupt(WHAT, "more epochs than the cap"));
        }
        if body.len() != count.saturating_mul(SIDECAR_EPOCH_BYTES) {
            return Err(corrupt(WHAT, "the epochs do not fill the value"));
        }
        // ALLOC: one entry an epoch, at most MAX_SIDECAR_EPOCHS (checked above).
        let mut epochs = Vec::with_capacity(count);
        for chunk in body.chunks_exact(SIDECAR_EPOCH_BYTES) {
            let (head, bitmap) = chunk.split_at(16);
            let number = |from: usize| {
                head.get(from..from.saturating_add(8))
                    .and_then(|b| <[u8; 8]>::try_from(b).ok())
                    .map(u64::from_be_bytes)
                    .ok_or(corrupt(WHAT, "a short epoch"))
            };
            let mut epoch = SidecarEpoch::new(number(0)?, number(8)?);
            epoch.collected = <[u8; BITMAP_BYTES]>::try_from(bitmap)
                .map_err(|_| corrupt(WHAT, "a short bitmap"))?;
            epochs.push(epoch);
        }
        Ok(Self { epochs })
    }

    /// The register value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for more epochs than the cap.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        if self.epochs.len() > MAX_SIDECAR_EPOCHS {
            return Err(corrupt(WHAT, "more epochs than the cap"));
        }
        let count =
            u32::try_from(self.epochs.len()).map_err(|_| corrupt(WHAT, "too many epochs"))?;
        // ALLOC: one sidecar value, 144 bytes an epoch, at most the 256 KiB cap.
        let mut out = Vec::with_capacity(
            PREFIX_BYTES.saturating_add(self.epochs.len().saturating_mul(SIDECAR_EPOCH_BYTES)),
        );
        out.push(VERSION);
        out.extend_from_slice(&count.to_be_bytes());
        for e in &self.epochs {
            out.extend_from_slice(&e.epoch.to_be_bytes());
            out.extend_from_slice(&e.high.to_be_bytes());
            out.extend_from_slice(&e.collected);
        }
        Ok(out)
    }
}
