//! The record of a departed node's packs.
//!
//! Rust only. `packs/index/retired/<node>` is written once, by the collector
//! that first lists a departed node's `packs/<node>/` prefix: a version byte
//! (1), a state byte (0 listed, 1 over the key cap), and for a listing the
//! bytes of a [`PackSidecar`] whose bitmaps mark every ordinal that was *not*
//! listed, so the judge reads it as a sidecar in which only the packs that
//! exist are left to collect. An over-cap record holds nothing after its
//! state: the node is never listed again.

use crate::error::{FormatError, corrupt};
use crate::keys::{check_base, check_segment};
use crate::pack_sidecar::{ORDINALS_PER_EPOCH, PackSidecar, SidecarEpoch};

const WHAT: &str = "node pack retirement";
const VERSION: u8 = 1;

/// The key of `node`'s retirement record under `base` (empty, or ending in
/// `/`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base or node id that cannot be part of a key.
pub fn pack_retired_key(base: &str, node: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_segment(node)?;
    // ALLOC: one key, a base, a fixed prefix and a node id.
    Ok([base, "packs/index/retired/", node].concat())
}

/// What listing a departed node's packs found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackRetired {
    over_cap: bool,
    sidecar: PackSidecar,
}

impl PackRetired {
    /// The node held more packs than the cap: nothing is recorded but that.
    pub const fn over_cap() -> Self {
        Self {
            over_cap: true,
            // ALLOC: an empty sidecar, no epochs.
            sidecar: PackSidecar::new(Vec::new()),
        }
    }

    /// The packs a listing found, as `(epoch, ordinal)` pairs in any order.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for an ordinal past an epoch.
    pub fn listed(packs: &[(u64, u64)]) -> Result<Self, FormatError> {
        // ALLOC: one entry an epoch the listing names, at most its pairs.
        let mut by_epoch: std::collections::BTreeMap<u64, Vec<u64>> =
            std::collections::BTreeMap::new();
        for &(epoch, ordinal) in packs {
            if ordinal >= ORDINALS_PER_EPOCH {
                return Err(corrupt(WHAT, "an ordinal past its epoch"));
            }
            by_epoch.entry(epoch).or_default().push(ordinal);
        }
        // ALLOC: one entry an epoch, at most the pairs.
        let mut epochs = Vec::with_capacity(by_epoch.len());
        for (epoch, ordinals) in by_epoch {
            let high = ordinals
                .iter()
                .copied()
                .max()
                .map_or(0, |m| m.saturating_add(1));
            let mut e = SidecarEpoch::new(epoch, high);
            for ordinal in 0..high {
                if !ordinals.contains(&ordinal) {
                    e.mark(ordinal);
                }
            }
            epochs.push(e);
        }
        Ok(Self {
            over_cap: false,
            sidecar: PackSidecar::new(epochs),
        })
    }

    /// Whether the node was over the cap and is left unlisted.
    pub const fn is_over_cap(&self) -> bool {
        self.over_cap
    }

    /// The sidecar the judge works from: only the listed packs unmarked.
    pub const fn sidecar(&self) -> &PackSidecar {
        &self.sidecar
    }

    /// The register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for more epochs than a sidecar holds.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        if self.over_cap {
            // ALLOC: the two bytes of an over-cap record.
            return Ok(vec![VERSION, 1]);
        }
        let body = self.sidecar.encode()?;
        // ALLOC: one value, two bytes and a sidecar of at most its cap.
        let mut out = Vec::with_capacity(body.len().saturating_add(2));
        out.extend_from_slice(&[VERSION, 0]);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Parses a register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Unsupported`] for another version,
    /// [`FormatError::Corrupt`] for a value that is not a record.
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let Some((&[version, state], rest)) = bytes.split_first_chunk::<2>() else {
            return Err(corrupt(WHAT, "shorter than its prefix"));
        };
        if version != VERSION {
            return Err(FormatError::Unsupported {
                what: WHAT,
                version: version.into(),
            });
        }
        match state {
            0 => Ok(Self {
                over_cap: false,
                sidecar: PackSidecar::parse(rest)?,
            }),
            1 if rest.is_empty() => Ok(Self::over_cap()),
            _ => Err(corrupt(
                WHAT,
                "an unknown state, or an over-cap record with a body",
            )),
        }
    }
}
