//! A register: a blob whose body is a generation, then a value.
//!
//! Ported from `server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobRegisters.java`
//! (`read`) on huyz0/OpenSearch `feature/serverlessshell`, whose S3, GCS and
//! Azure containers lay a register out the same way: the generation as a
//! big-endian 64-bit integer, then the value. Commit manifests and index
//! descriptors are register values.

use crate::error::{FormatError, len64};

/// Bytes before a register's value.
pub const GENERATION_BYTES: usize = 8;

/// A register's generation and value, borrowing from its blob's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Register<'a> {
    /// The generation a compare-and-swap names.
    pub generation: u64,
    /// The value.
    pub value: &'a [u8],
}

impl<'a> Register<'a> {
    /// Splits a register blob's bytes.
    ///
    /// # Errors
    ///
    /// [`FormatError::Truncated`] for fewer than [`GENERATION_BYTES`] bytes,
    /// which the Java refuses as corrupt.
    pub fn parse(blob: &'a [u8]) -> Result<Self, FormatError> {
        let (Some(generation), Some(value)) =
            (blob.get(..GENERATION_BYTES), blob.get(GENERATION_BYTES..))
        else {
            return Err(FormatError::Truncated {
                what: "register",
                needed: len64(GENERATION_BYTES),
                have: len64(blob.len()),
            });
        };
        let generation = u64::from_be_bytes(generation.try_into().unwrap_or_default());
        Ok(Self { generation, value })
    }
}
