//! A node's snapshot of a document set, kept while the set has not moved.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/template/TemplateCache.java`
//! and `TemplateStore.all`. Index creation reads every template to find the
//! matching ones, so it must cost a register read and not a read a template:
//! one GET of the marker says whether the copy is current, and a changed set
//! reads only the documents whose content key moved. One immutable snapshot
//! is replaced whole, so a reader never sees one version labelled with
//! another's documents.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::doc_set::DocSet;
use crate::error::MetaError;

/// The most bytes a cache keeps, as the Java's `MAX_CACHED_BYTES`.
pub const DEFAULT_DOC_CACHE_BYTES: usize = 8 << 20;

/// What a document costs to keep besides its bytes and names, as the Java's
/// `retainedBytes`.
const OVERHEAD_BYTES: usize = 128;

#[derive(Debug)]
struct Doc {
    name: String,
    content: String,
    bytes: Arc<[u8]>,
}

/// One consistent reading of a set: the marker's version and every
/// document, in name order.
#[derive(Debug)]
pub struct DocSnapshot {
    version: i64,
    docs: Vec<Doc>,
}

impl DocSnapshot {
    /// The marker's version this was read at.
    pub fn version(&self) -> i64 {
        self.version
    }

    /// Every document, in name order, as name and bytes.
    pub fn docs(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.docs.iter().map(|d| (d.name.as_str(), &*d.bytes))
    }

    /// The document `name`.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.docs.iter().find(|d| d.name == name).map(|d| &*d.bytes)
    }

    /// What keeping this costs, as the Java counts it: each document's
    /// bytes, its name and key, and a fixed overhead.
    pub fn retained_bytes(&self) -> usize {
        self.docs.iter().fold(0usize, |sum, d| {
            sum.saturating_add(OVERHEAD_BYTES)
                .saturating_add(d.name.len())
                .saturating_add(d.content.len())
                .saturating_add(d.bytes.len())
        })
    }

    fn is_current(&self, version: i64, entries: &[(&str, &str)]) -> bool {
        self.version == version
            && self.docs.len() == entries.len()
            && self
                .docs
                .iter()
                .zip(entries)
                .all(|(d, (name, content))| d.name == *name && d.content == *content)
    }
}

/// The snapshot a node holds of one set: share one between every use of the
/// set on the node.
pub struct DocSetCache {
    budget_bytes: usize,
    kept: Mutex<Option<Arc<DocSnapshot>>>,
}

impl Default for DocSetCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DocSetCache {
    /// An empty cache keeping up to [`DEFAULT_DOC_CACHE_BYTES`].
    pub const fn new() -> Self {
        Self::with_budget(DEFAULT_DOC_CACHE_BYTES)
    }

    /// An empty cache keeping a snapshot only if it retains at most
    /// `budget_bytes`; a larger set is read through every time.
    pub const fn with_budget(budget_bytes: usize) -> Self {
        Self {
            budget_bytes,
            kept: Mutex::new(None),
        }
    }

    /// The lock, kept usable after a panic elsewhere: the snapshot is only
    /// ever replaced whole, and the worst a lost one does is a read through.
    fn lock(&self) -> MutexGuard<'_, Option<Arc<DocSnapshot>>> {
        self.kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl DocSet {
    /// Every document of the set as one snapshot: one GET of the marker, and
    /// when the set has moved since `cache` took its copy, one more for each
    /// document whose content key changed. A snapshot over the cache's
    /// budget is returned and not kept. A document dropped by a change
    /// while reading restarts from the marker once.
    ///
    /// # Errors
    ///
    /// As [`DocSet::get`].
    pub fn snapshot(
        &self,
        cache: &DocSetCache,
        marker_buf: &mut [u8],
        body_buf: &mut [u8],
    ) -> Result<Arc<DocSnapshot>, MetaError> {
        let mut missing = None;
        for _ in 0..2 {
            let read = self.read(marker_buf)?;
            // ALLOC: one entry each, at most MAX_DOCS.
            let mut entries: Vec<(&str, &str)> = read
                .marker
                .entries
                .iter()
                .map(|(n, c)| (n.as_ref(), c.as_ref()))
                .collect(); // ALLOC: the entries above, at most MAX_DOCS
            entries.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
            // ALLOC: one shared pointer, cloned.
            let held = cache.lock().clone();
            if let Some(held) = &held
                && held.is_current(read.marker.version, &entries)
            {
                return Ok(Arc::clone(held));
            }
            // ALLOC: one document each, at most MAX_DOCS.
            let mut docs = Vec::with_capacity(entries.len());
            let mut complete = true;
            for (name, content) in entries {
                let reused = held.as_ref().and_then(|h| {
                    h.docs
                        .iter()
                        .find(|d| d.name == name && d.content == content)
                });
                let bytes = if let Some(doc) = reused {
                    Arc::clone(&doc.bytes)
                } else if let Some(len) = self.read_content(content, body_buf)? {
                    // ALLOC: one document, at most MAX_DOC_BYTES, kept.
                    Arc::from(body_buf.get(..len).unwrap_or_default())
                } else {
                    // ALLOC: one name, once, on a damaged set.
                    missing = Some(name.to_owned());
                    complete = false;
                    break;
                };
                // ALLOC: one name and key each, at most MAX_DOCS.
                docs.push(Doc {
                    name: name.to_owned(),
                    content: content.to_owned(),
                    bytes,
                });
            }
            if !complete {
                continue;
            }
            // ALLOC: one snapshot, shared.
            let snapshot = Arc::new(DocSnapshot {
                version: read.marker.version,
                docs,
            });
            *cache.lock() =
                (snapshot.retained_bytes() <= cache.budget_bytes).then(|| Arc::clone(&snapshot));
            return Ok(snapshot);
        }
        Err(MetaError::MissingContent(missing.unwrap_or_default()))
    }
}
