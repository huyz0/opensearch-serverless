//! A node's pack ledger: which of its packs may still exist, so collection
//! can name every one without listing.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/store/NodePackLedger.java`
//! (`encode`, `decode`). The register at `packs/<node>/ledger` holds a version
//! byte (1), an epoch count (i32) and per epoch the epoch, the ordinal below
//! which every pack is gone, and one past the highest ordinal recorded, each an
//! i64, all big-endian. Stricter than the Java: bytes left over are refused.

use crate::error::{FormatError, corrupt};

const WHAT: &str = "node pack ledger";
const VERSION: u8 = 1;
const EPOCH_BYTES: usize = 24;
const PREFIX_BYTES: usize = 5;

/// One epoch's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackEpoch {
    /// The epoch.
    pub epoch: u64,
    /// Every ordinal below this is gone.
    pub collected_below: u64,
    /// One past the highest ordinal recorded as used; packs may exist a
    /// little past it.
    pub high: u64,
}

/// A parsed ledger: every epoch, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePackLedger {
    epochs: Vec<PackEpoch>,
}

impl NodePackLedger {
    /// Parses a ledger register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a short value, a negative number or
    /// bytes left over, [`FormatError::Unsupported`] for another version.
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
        let count = usize::try_from(i32::from_be_bytes(*count))
            .map_err(|_| corrupt(WHAT, "a negative epoch count"))?;
        if body.len() != count.saturating_mul(EPOCH_BYTES) {
            return Err(corrupt(WHAT, "the epochs do not fill the value"));
        }
        // ALLOC: one entry an epoch, at most the value's length / 24 (checked above).
        let mut epochs = Vec::with_capacity(count);
        for chunk in body.chunks_exact(EPOCH_BYTES) {
            let number = |from: usize| {
                chunk
                    .get(from..from.saturating_add(8))
                    .and_then(|b| <[u8; 8]>::try_from(b).ok())
                    .and_then(|b| u64::try_from(i64::from_be_bytes(b)).ok())
                    .ok_or(corrupt(WHAT, "a negative number"))
            };
            epochs.push(PackEpoch {
                epoch: number(0)?,
                collected_below: number(8)?,
                high: number(16)?,
            });
        }
        Ok(Self { epochs })
    }

    /// The epochs, oldest first.
    pub fn epochs(&self) -> &[PackEpoch] {
        &self.epochs
    }
}

/// The register value for `epochs`, as the Java writes it.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a number past `i64` or too many epochs.
pub fn encode(epochs: &[PackEpoch]) -> Result<Vec<u8>, FormatError> {
    let count = i32::try_from(epochs.len()).map_err(|_| corrupt(WHAT, "too many epochs"))?;
    // ALLOC: one ledger value, 24 bytes an epoch.
    let mut out =
        Vec::with_capacity(PREFIX_BYTES.saturating_add(epochs.len().saturating_mul(EPOCH_BYTES)));
    out.push(VERSION);
    out.extend_from_slice(&count.to_be_bytes());
    for e in epochs {
        for v in [e.epoch, e.collected_below, e.high] {
            let v = i64::try_from(v).map_err(|_| corrupt(WHAT, "a number past i64"))?;
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
    Ok(out)
}
