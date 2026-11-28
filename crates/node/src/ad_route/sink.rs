//! The seam a run's result documents leave through: the node's ordinary
//! write path, behind one call. Shaped as the host API's `DocSink` so the
//! node's implementation replaces a test's.

use serde_json::Value;

/// One document: an optional id and its source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// The document id, or none for a generated one.
    pub id: Option<String>,
    /// The JSON source.
    pub source: Vec<u8>,
}

impl Doc {
    /// A document with no id whose source is `value`.
    #[must_use]
    pub fn of(value: &Value) -> Self {
        Self {
            id: None,
            // ALLOC: one result document's text, a few hundred bytes.
            source: value.to_string().into_bytes(),
        }
    }
}

/// What an index call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexReport {
    /// Documents written.
    pub indexed: usize,
    /// Documents the index refused.
    pub failed: usize,
}

/// Why an index call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError(pub String);

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Writes documents into an index through the node's write path.
pub trait DocSink: Send + Sync {
    /// Indexes `docs` into `index`.
    ///
    /// # Errors
    ///
    /// [`HostError`] when the write path could not take them.
    fn index(&self, index: &str, docs: &[Doc]) -> Result<IndexReport, HostError>;
}
