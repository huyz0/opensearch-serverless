//! Where a published segment file's bytes are, as a commit manifest records it.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/store/FileLocation.java`.
//! Three forms, any of which one manifest may mix:
//!
//! - `t=<term>`: a blob of its own, named after the file, in a term container;
//! - `t=<term>/<blob>#<offset>:<length>`: a range of a segment blob (or an
//!   older multi-segment pack) in a term container;
//! - `@<node>/e=<epoch>/<ordinal>#<offset>:<length>`: a range of a node pack,
//!   outside the shard, with epoch and ordinal as 20 zero-padded digits.

use crate::error::{FormatError, corrupt};

const WHAT: &str = "file location";

/// What starts a node pack's recorded location; no term container's name does.
pub const NODE_PACK_PREFIX: &str = "@";

/// A parsed location, borrowing from the manifest's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLocation<'a> {
    /// A blob of its own named after the file, in term container `term_dir`.
    Whole {
        /// The term container, `t=<term>`.
        term_dir: &'a str,
    },
    /// A range of `blob` in term container `term_dir`.
    Packed {
        /// The term container.
        term_dir: &'a str,
        /// The blob's name in it.
        blob: &'a str,
        /// Where the file starts in the blob.
        offset: u64,
        /// The file's length.
        length: u64,
    },
    /// A range of a node pack.
    NodePacked {
        /// The node that wrote the pack.
        node: &'a str,
        /// The pack's epoch.
        epoch: u64,
        /// The pack's slot in its epoch.
        ordinal: u64,
        /// Where the file starts in the pack.
        offset: u64,
        /// The file's length.
        length: u64,
    },
}

impl<'a> FileLocation<'a> {
    /// Parses a location as a manifest records it, with the Java's rules: a
    /// node-packed location must re-encode to exactly its recorded text.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a value no reader could follow.
    pub fn parse(recorded: &'a str) -> Result<Self, FormatError> {
        if recorded.is_empty() {
            return Err(corrupt(WHAT, "a location is empty"));
        }
        if let Some(rest) = recorded.strip_prefix(NODE_PACK_PREFIX) {
            return parse_node_packed(rest);
        }
        let Some((term_dir, range)) = recorded.split_once('/') else {
            return Ok(Self::Whole { term_dir: recorded });
        };
        // The Java finds the first '#' after the slash, then the first ':' after it.
        let Some((blob, numbers)) = range.split_once('#') else {
            return Err(corrupt(WHAT, "a packed location has no '#'"));
        };
        let Some((offset, length)) = numbers.split_once(':') else {
            return Err(corrupt(WHAT, "a packed location has no ':'"));
        };
        // An empty offset or length is refused by `java_long`.
        if term_dir.is_empty() || blob.is_empty() {
            return Err(corrupt(WHAT, "a packed location has an empty part"));
        }
        Ok(Self::Packed {
            term_dir,
            blob,
            offset: java_long(offset)?,
            length: java_long(length)?,
        })
    }

    /// The text a manifest records for this location: what [`parse`] reads.
    ///
    /// [`parse`]: FileLocation::parse
    pub fn encode(&self) -> String {
        match *self {
            Self::Whole { term_dir } => {
                // ALLOC: one location's text, a few dozen bytes, once a file a publish.
                term_dir.to_owned()
            }
            Self::Packed {
                term_dir,
                blob,
                offset,
                length,
            } => {
                // ALLOC: one location's text, a few dozen bytes, once a file a publish.
                format!("{term_dir}/{blob}#{offset}:{length}")
            }
            Self::NodePacked {
                node,
                epoch,
                ordinal,
                offset,
                length,
            } => {
                // ALLOC: one location's text, under a hundred bytes, once a file a publish.
                format!("{NODE_PACK_PREFIX}{node}/e={epoch:020}/{ordinal:020}#{offset}:{length}")
            }
        }
    }

    /// The file's range in its blob, or `None` for a blob of its own.
    pub fn range(&self) -> Option<(u64, u64)> {
        match *self {
            Self::Whole { .. } => None,
            Self::Packed { offset, length, .. } | Self::NodePacked { offset, length, .. } => {
                Some((offset, length))
            }
        }
    }
}

/// `<node>/e=<epoch>/<ordinal>#<offset>:<length>`, after the `@`.
fn parse_node_packed(rest: &str) -> Result<FileLocation<'_>, FormatError> {
    let Some((path, numbers)) = rest.split_once('#') else {
        return Err(corrupt(WHAT, "a node-packed location has no '#'"));
    };
    let Some((node, epoch_and_ordinal)) = path.split_once('/') else {
        return Err(corrupt(WHAT, "a node-packed location has no node"));
    };
    let Some((epoch_dir, ordinal)) = epoch_and_ordinal.split_once('/') else {
        return Err(corrupt(WHAT, "a node-packed location has no ordinal"));
    };
    let Some(epoch) = epoch_dir.strip_prefix("e=") else {
        return Err(corrupt(
            WHAT,
            "a node-packed location's epoch does not start with e=",
        ));
    };
    let Some((offset, length)) = numbers.split_once(':') else {
        return Err(corrupt(WHAT, "a node-packed location has no ':'"));
    };
    if node.is_empty() {
        return Err(corrupt(WHAT, "a node-packed location has an empty part"));
    }
    // Re-encoding must give back the recorded text: twenty digits each, no
    // signs, no extra slashes.
    if !is_padded(epoch) || !is_padded(ordinal) {
        return Err(corrupt(
            WHAT,
            "a node pack's epoch and ordinal are twenty digits",
        ));
    }
    let offset_value = java_long(offset)?;
    let length_value = java_long(length)?;
    if !is_canonical(offset, offset_value) || !is_canonical(length, length_value) {
        return Err(corrupt(
            WHAT,
            "a node-packed range is not written as the shell writes it",
        ));
    }
    Ok(FileLocation::NodePacked {
        node,
        epoch: java_long(epoch)?,
        ordinal: java_long(ordinal)?,
        offset: offset_value,
        length: length_value,
    })
}

fn is_padded(digits: &str) -> bool {
    digits.len() == 20 && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `text` is how Java's `Long.toString` writes `value`.
fn is_canonical(text: &str, value: u64) -> bool {
    let mut buffer = [0u8; 20];
    text.as_bytes() == decimal(value, &mut buffer)
}

/// `value` in decimal, into `buffer`.
pub(crate) fn decimal(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut at = buffer.len();
    loop {
        at = at.saturating_sub(1);
        if let Some(slot) = buffer.get_mut(at) {
            // `value % 10` is below ten, so the cast and the add cannot overflow.
            *slot = b'0'.saturating_add(u8::try_from(value % 10).unwrap_or(0));
        }
        value /= 10;
        if value == 0 || at == 0 {
            break;
        }
    }
    buffer.get(at..).unwrap_or_default()
}

/// A non-negative number as Java's `Long.parseLong` reads one, which accepts
/// a leading `+` and leading zeros. The Java's `FileLocation` then refuses a
/// negative offset or length, so a `-` is refused here.
fn java_long(text: &str) -> Result<u64, FormatError> {
    let digits = text.strip_prefix('+').unwrap_or(text);
    // `u64::from_str` takes a leading `+`, which was stripped once already.
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(corrupt(
            WHAT,
            "a location's number is not a non-negative integer",
        ));
    }
    // An empty or over-long number fails here.
    let value: u64 = digits
        .parse()
        .map_err(|_| corrupt(WHAT, "a location's number is out of range"))?;
    if value > i64::MAX as u64 {
        return Err(corrupt(WHAT, "a location's number is out of range"));
    }
    Ok(value)
}
