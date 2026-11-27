//! The seeding register: where the one-time walk of `packs/` has got to.
//!
//! Rust only. `packs/index/seeded` holds a version byte (1), a state byte
//! (0 in progress, 1 done), a cursor length (u16) and the cursor's bytes,
//! big-endian. The cursor is the last child of `packs/` whose ledger has
//! been read, so a crashed walk resumes after it; a done state has none.

use crate::error::{FormatError, corrupt};

const WHAT: &str = "node pack seed";
const VERSION: u8 = 1;
const PREFIX_BYTES: usize = 4;

/// The longest cursor: a key's bound.
const MAX_CURSOR_BYTES: usize = 1024;

/// The key of the seeding register under `base` (empty, or ending in `/`).
pub fn pack_seed_key(base: &str) -> String {
    // ALLOC: one key, a base and a fixed name.
    [base, "packs/index/seeded"].concat()
}

/// Where the seeding walk has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSeed {
    done: bool,
    cursor: Option<String>,
}

impl PackSeed {
    /// A walk begun, with nothing read yet.
    pub const fn begun() -> Self {
        Self {
            done: false,
            cursor: None,
        }
    }

    /// A walk in progress that has read up to and including `child`.
    pub fn after(child: &str) -> Self {
        Self {
            done: false,
            // ALLOC: one cursor, at most MAX_CURSOR_BYTES (checked on encode).
            cursor: Some(child.to_owned()),
        }
    }

    /// A walk that reached the end.
    pub const fn finished() -> Self {
        Self {
            done: true,
            cursor: None,
        }
    }

    /// Whether the walk reached the end.
    pub const fn done(&self) -> bool {
        self.done
    }

    /// The last child read, if any.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// The register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] for a cursor past its bound.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        let cursor = self.cursor.as_deref().unwrap_or("");
        if cursor.len() > MAX_CURSOR_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_CURSOR_BYTES as u64,
                actual: cursor.len() as u64,
            });
        }
        let len = u16::try_from(cursor.len()).map_err(|_| corrupt(WHAT, "a cursor too long"))?;
        // ALLOC: one value, a prefix and a cursor of at most MAX_CURSOR_BYTES.
        let mut out = Vec::with_capacity(PREFIX_BYTES.saturating_add(cursor.len()));
        out.push(VERSION);
        out.push(u8::from(self.done));
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(cursor.as_bytes());
        Ok(out)
    }

    /// Parses a register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Unsupported`] for another version,
    /// [`FormatError::Corrupt`] for a value that is not a state.
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let Some((&[version, state, hi, lo], rest)) = bytes.split_first_chunk::<PREFIX_BYTES>()
        else {
            return Err(corrupt(WHAT, "shorter than its prefix"));
        };
        if version != VERSION {
            return Err(FormatError::Unsupported {
                what: WHAT,
                version: version.into(),
            });
        }
        let len = usize::from(u16::from_be_bytes([hi, lo]));
        if len > MAX_CURSOR_BYTES || rest.len() != len {
            return Err(corrupt(WHAT, "the cursor does not fill the value"));
        }
        let cursor = std::str::from_utf8(rest).map_err(|_| corrupt(WHAT, "a cursor not UTF-8"))?;
        match (state, cursor.is_empty()) {
            (0, true) => Ok(Self::begun()),
            (0, false) => Ok(Self::after(cursor)),
            (1, true) => Ok(Self::finished()),
            _ => Err(corrupt(
                WHAT,
                "an unknown state, or a finished walk with a cursor",
            )),
        }
    }
}
