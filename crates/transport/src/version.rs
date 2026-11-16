//! The OpenSearch version this node speaks on the wire, and which peers it
//! accepts.
//!
//! OpenSearch writes a version as its id XOR `0x08000000` (`Version.MASK`),
//! so its ids sort after the predecessor's; the id `major * 1,000,000 +
//! minor * 10,000 + revision * 100 + build` of 3.10.0 is `3100099`.

use crate::error::WireError;
use crate::stream::{Input, Output};

/// OpenSearch's `Version.MASK`.
const MASK: u32 = 0x0800_0000;

/// A version as OpenSearch numbers it, unmasked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WireVersion(u32);

impl WireVersion {
    /// The Java shell's version, which this node announces: 3.10.0.
    pub const SHELL: Self = Self(3_100_099);

    /// 3.10.0's minimum wire-compatible version (the last 2.x minor, 2.19.0),
    /// which the TCP handshake request carries.
    pub const MIN_COMPAT: Self = Self(2_190_099);

    /// The versions a peer may run: those the recorded fixtures cover.
    pub const ACCEPTED: [Self; 1] = [Self::SHELL];

    /// The version with OpenSearch id `id` (unmasked, `3100099` for 3.10.0).
    pub const fn from_id(id: u32) -> Self {
        Self(id)
    }

    /// The unmasked id.
    pub const fn id(self) -> u32 {
        self.0
    }

    /// The id as the wire carries it: XOR `0x08000000`.
    pub const fn wire_id(self) -> u32 {
        self.0 ^ MASK
    }

    /// The version a wire id names.
    pub const fn from_wire_id(wire: u32) -> Self {
        Self(wire ^ MASK)
    }

    /// Whether a peer at this version is accepted.
    pub fn accepted(self) -> bool {
        Self::ACCEPTED.contains(&self)
    }

    /// Refuses a peer at a version this node does not speak.
    ///
    /// # Errors
    ///
    /// [`WireError::UnsupportedVersion`] unless [`Self::accepted`].
    pub fn check_accepted(self) -> Result<Self, WireError> {
        if self.accepted() {
            Ok(self)
        } else {
            Err(WireError::UnsupportedVersion { id: self.0 })
        }
    }

    /// `writeVersion`: the wire id as a vint.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.vint(i32::from_ne_bytes(self.wire_id().to_ne_bytes()))
    }

    /// `readVersion`.
    ///
    /// # Errors
    ///
    /// As [`Input::vint`].
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        let wire = input.vint("a version")?;
        Ok(Self::from_wire_id(u32::from_ne_bytes(wire.to_ne_bytes())))
    }
}
