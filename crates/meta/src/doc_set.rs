//! Document sets: index and component templates, ingest and search
//! pipelines, each a few documents read by name.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/template/TemplateStore.java`.
//! A document's bytes are an immutable content blob under a unique key; one
//! marker register names them, so a change is one compare-and-swap of the
//! marker and a reader that sees the new marker sees the new set. A blob
//! written before a crash is never named, so never seen. Nothing lists: the
//! marker holds the names, and the count is bounded on the way in by
//! [`MAX_DOCS`].

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore, ByteRange, SliceSource, SwapOutcome};
use oss_format::{
    DocKind, DocSetMarker, MAX_DOC_NAME_BYTES, MAX_DOCS, doc_content_key, doc_set_marker_key,
};

use crate::error::MetaError;
use crate::pit::new_id;

/// How many times a change reads the marker and swaps before giving up to
/// contention, as the Java's.
const ATTEMPTS: u32 = 10;

mod update;

/// One set of documents under a deployment's base.
pub struct DocSet {
    store: Arc<dyn BlobStore>,
    base: String,
    kind: DocKind,
    marker_key: String,
    /// Whether the set must have been created: a read that finds no marker
    /// is [`MetaError::NoSuchSet`] and not an empty set.
    must_exist: bool,
}

/// What a [`DocSet::put_counted`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Put {
    /// The set's change count after the put.
    pub version: i64,
    /// Whether the name was already there.
    pub replaced: bool,
}

/// The marker as read, and the generation to swap it from.
pub(crate) struct Read<'a> {
    pub(crate) marker: DocSetMarker<'a>,
    generation: u64,
}

impl DocSet {
    /// The set of `kind` under `base` (empty, or ending in `/`) of `store`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end
    /// in `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, kind: DocKind) -> Result<Self, MetaError> {
        Ok(Self {
            store,
            marker_key: doc_set_marker_key(base, kind)?,
            // ALLOC: one copy of the base, once a set.
            base: base.to_owned(),
            kind,
            must_exist: false,
        })
    }

    /// As [`DocSet::new`], for a set that is created by [`DocSet::create`]:
    /// every other call on one that was never created is
    /// [`MetaError::NoSuchSet`], at no more requests than the marker's read.
    ///
    /// # Errors
    ///
    /// As [`DocSet::new`].
    pub fn new_created(
        store: Arc<dyn BlobStore>,
        base: &str,
        kind: DocKind,
    ) -> Result<Self, MetaError> {
        let mut set = Self::new(store, base, kind)?;
        set.must_exist = true;
        Ok(set)
    }

    /// Creates the empty set: one marker, version 1. Returns whether it
    /// was made, `false` when it already existed.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`]; [`MetaError::Blob`].
    pub fn create(&self) -> Result<bool, MetaError> {
        self.swap(&DocSetMarker::new(1), ABSENT_GENERATION)
    }

    /// Removes the set's marker, which ends the set (its documents must
    /// have been removed first).
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`].
    pub fn destroy(&self) -> Result<(), MetaError> {
        Ok(self.store.delete(&self.marker_key)?)
    }

    pub(crate) fn read<'a>(&self, buf: &'a mut [u8]) -> Result<Read<'a>, MetaError> {
        match self.store.read_register(&self.marker_key, buf)? {
            None if self.must_exist => Err(MetaError::NoSuchSet),
            None => Ok(Read {
                marker: DocSetMarker::new(0),
                generation: ABSENT_GENERATION,
            }),
            Some(read) => Ok(Read {
                marker: DocSetMarker::parse(buf.get(..read.len).unwrap_or_default())?,
                generation: read.generation,
            }),
        }
    }

    /// The number of changes the set has had, 0 for none: one request.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the store fails or the marker does not fit
    /// `marker_buf`; [`MetaError::Format`] for a malformed marker.
    pub fn version(&self, marker_buf: &mut [u8]) -> Result<i64, MetaError> {
        Ok(self.read(marker_buf)?.marker.version)
    }

    /// The names in the set, in order: one request.
    ///
    /// # Errors
    ///
    /// As [`DocSet::version`].
    pub fn names(&self, marker_buf: &mut [u8]) -> Result<Vec<String>, MetaError> {
        let read = self.read(marker_buf)?;
        // ALLOC: one name each, at most MAX_DOCS.
        let mut names: Vec<String> = read
            .marker
            .entries
            .iter()
            // ALLOC: one name each, at most MAX_DOCS
            .map(|(name, _)| name.as_ref().to_owned())
            .collect();
        names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        Ok(names)
    }

    /// Reads the document `name` into the front of `body_buf` and returns
    /// its length, or `None` when the set has none: the marker and then the
    /// document, two requests. A document the marker names that is gone was
    /// dropped by a change since; the marker is read once more.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidDocName`]; [`MetaError::MissingContent`] when it
    /// is still gone; [`MetaError::Blob`] when the store fails or the
    /// document does not fit `body_buf`; [`MetaError::Format`].
    pub fn get(
        &self,
        name: &str,
        marker_buf: &mut [u8],
        body_buf: &mut [u8],
    ) -> Result<Option<usize>, MetaError> {
        validate_doc_name(name, self.kind)?;
        for _ in 0..2 {
            let read = self.read(marker_buf)?;
            let Some(content) = read.marker.content(name) else {
                return Ok(None);
            };
            if let Some(len) = self.read_content(content, body_buf)? {
                return Ok(Some(len));
            }
        }
        // ALLOC: one name, once, on a damaged set
        Err(MetaError::MissingContent(name.to_owned()))
    }

    /// Visits every document in name order: the marker, then one request a
    /// document, one after another (one buffer). Bounded by [`MAX_DOCS`] on
    /// the way in, never by truncating an answer. When a change drops a
    /// document mid-way the walk starts again from the marker, so `visit`
    /// may be called twice for a name, the second time from the newer set.
    ///
    /// # Errors
    ///
    /// As [`DocSet::get`].
    pub fn for_each(
        &self,
        marker_buf: &mut [u8],
        body_buf: &mut [u8],
        visit: &mut dyn FnMut(&str, &[u8]),
    ) -> Result<(), MetaError> {
        for attempt in 0..2 {
            let read = self.read(marker_buf)?;
            // ALLOC: one entry each, at most MAX_DOCS.
            let mut order: Vec<(&str, &str)> = read
                .marker
                .entries
                .iter()
                .map(|(n, c)| (n.as_ref(), c.as_ref()))
                // ALLOC: one entry each, at most MAX_DOCS
                .collect();
            order.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
            let mut complete = true;
            for (name, content) in order {
                let Some(len) = self.read_content(content, body_buf)? else {
                    if attempt == 1 {
                        // ALLOC: one name, once, on a damaged set
                        return Err(MetaError::MissingContent(name.to_owned()));
                    }
                    complete = false;
                    break;
                };
                visit(name, body_buf.get(..len).unwrap_or_default());
            }
            if complete {
                return Ok(());
            }
        }
        Ok(())
    }

    pub(crate) fn read_content(
        &self,
        content: &str,
        body_buf: &mut [u8],
    ) -> Result<Option<usize>, MetaError> {
        let key = doc_content_key(&self.base, self.kind, content)?;
        let mut scratch = [0u8; 4096];
        let mut filled = 0usize;
        let mut sink = |chunk: &[u8]| -> Result<(), BlobError> {
            let end = filled.saturating_add(chunk.len());
            let to = body_buf
                .get_mut(filled..end)
                .ok_or(BlobError::InvalidArgument(
                    "a document does not fit the buffer",
                ))?;
            to.copy_from_slice(chunk);
            filled = end;
            Ok(())
        };
        match self
            .store
            .get(&key, ByteRange::Full, &mut scratch, &mut sink)
        {
            Ok(_) => Ok(Some(filled)),
            Err(BlobError::NotFound) => Ok(None),
            Err(other) => Err(other.into()),
        }
    }

    /// Stores `source` as `name`, replacing a document of that name unless
    /// `require_absent`. The bytes first under a new key, then one swap of
    /// the marker naming them; the replaced bytes go afterwards unless
    /// another name reads them. A lost swap reads the marker and tries again,
    /// up to 10 attempts, writing the bytes once.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidDocName`]; [`MetaError::InvalidArgument`] for a
    /// document over its kind's [`DocKind::max_doc_bytes`]; [`MetaError::AlreadyExists`] under
    /// `require_absent`; [`MetaError::TooManyDocs`] for a new name in a full
    /// set; [`MetaError::Contended`]; [`MetaError::Format`];
    /// [`MetaError::Blob`].
    pub fn put(
        &self,
        name: &str,
        source: &[u8],
        require_absent: bool,
        marker_buf: &mut [u8],
    ) -> Result<(), MetaError> {
        self.put_counted(name, source, require_absent, marker_buf)
            .map(|_| ())
    }

    /// As [`DocSet::put`], telling the change count after it and whether it
    /// replaced a document.
    ///
    /// # Errors
    ///
    /// As [`DocSet::put`], and [`MetaError::NoSuchSet`] for a set made by
    /// [`DocSet::new_created`] that was never created.
    pub fn put_counted(
        &self,
        name: &str,
        source: &[u8],
        require_absent: bool,
        marker_buf: &mut [u8],
    ) -> Result<Put, MetaError> {
        validate_doc_name(name, self.kind)?;
        if source.len() > self.kind.max_doc_bytes() {
            return Err(MetaError::InvalidArgument(
                "a document is over its size bound",
            ));
        }
        let mut content: Option<String> = None;
        for _ in 0..ATTEMPTS {
            let read = self.read(marker_buf);
            let Read {
                mut marker,
                generation,
            } = match read {
                Ok(read) => read,
                Err(e) => {
                    self.drop_unnamed(content.as_deref());
                    return Err(e);
                }
            };
            let exists = marker.content(name).is_some();
            let refusal = if require_absent && exists {
                Some(MetaError::AlreadyExists)
            } else if !exists && marker.entries.len() >= MAX_DOCS {
                Some(MetaError::TooManyDocs)
            } else {
                None
            };
            if let Some(refusal) = refusal {
                self.drop_unnamed(content.as_deref());
                return Err(refusal);
            }
            let fresh = match &content {
                Some(id) => id.clone(),
                None => {
                    let id = new_id()?;
                    self.write_content(&id, source)?;
                    content = Some(id.clone());
                    id
                }
            };
            let previous = marker.content(name).map(str::to_owned);
            let replaced = previous.is_some();
            marker.insert(name, &fresh);
            marker.version = marker.version.saturating_add(1);
            let swapped = match self.swap(&marker, generation) {
                Ok(swapped) => swapped,
                Err(e) => {
                    self.drop_unnamed(content.as_deref());
                    return Err(e);
                }
            };
            if swapped {
                if let Some(old) = previous
                    && old != fresh
                    && !marker.shares(&old)
                {
                    self.drop_content(&old);
                }
                return Ok(Put {
                    version: marker.version,
                    replaced,
                });
            }
        }
        self.drop_unnamed(content.as_deref());
        Err(MetaError::Contended)
    }

    /// Removes the document `name`: one swap of the marker, then its bytes
    /// unless another name reads them. Returns whether there was one.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidDocName`]; [`MetaError::Contended`];
    /// [`MetaError::Format`]; [`MetaError::Blob`].
    pub fn delete(&self, name: &str, marker_buf: &mut [u8]) -> Result<bool, MetaError> {
        validate_doc_name(name, self.kind)?;
        for _ in 0..ATTEMPTS {
            let Read {
                mut marker,
                generation,
            } = self.read(marker_buf)?;
            let Some(content) = marker.remove(name) else {
                return Ok(false);
            };
            let content = content.into_owned();
            marker.version = marker.version.saturating_add(1);
            if self.swap(&marker, generation)? {
                if !marker.shares(&content) {
                    self.drop_content(&content);
                }
                return Ok(true);
            }
        }
        Err(MetaError::Contended)
    }

    fn swap(&self, marker: &DocSetMarker<'_>, generation: u64) -> Result<bool, MetaError> {
        // ALLOC: one marker's worth, the most a set can write, per attempt.
        let mut buf = vec![0u8; oss_format::MAX_DOC_SET_MARKER_BYTES];
        let len = marker.write(&mut buf)?;
        let value = buf.get(..len).unwrap_or_default();
        let outcome = if generation == ABSENT_GENERATION {
            self.store.create_register(&self.marker_key, value)?
        } else {
            self.store
                .swap_register(&self.marker_key, generation, value)?
        };
        Ok(matches!(outcome, SwapOutcome::Applied(_)))
    }

    fn write_content(&self, id: &str, source: &[u8]) -> Result<(), MetaError> {
        let key = doc_content_key(&self.base, self.kind, id)?;
        let mut scratch = [0u8; 4096];
        self.store
            .put(&key, &mut SliceSource::new(source), &mut scratch)?;
        Ok(())
    }

    /// Deletes bytes no marker names. A failure costs storage and nothing
    /// else: an unnamed blob is never read.
    fn drop_content(&self, id: &str) {
        if let Ok(key) = doc_content_key(&self.base, self.kind, id) {
            let _ = self.store.delete(&key);
        }
    }

    fn drop_unnamed(&self, id: Option<&str>) {
        if let Some(id) = id {
            self.drop_content(id);
        }
    }
}

/// Refuses a name the Java refuses for a template or pipeline
/// (`Names.validateId`), with its message.
fn validate_doc_name(name: &str, kind: DocKind) -> Result<(), MetaError> {
    let what = match kind {
        DocKind::IndexTemplate | DocKind::ComponentTemplate => "template",
        DocKind::IngestPipeline | DocKind::SearchPipeline => "pipeline",
        DocKind::StoredScript => "script",
        DocKind::LtrStores => "store",
        DocKind::LtrFeatureSets => "feature set",
        DocKind::LtrModels | DocKind::KnnModels => "model",
        DocKind::IsmPolicies => "policy",
        DocKind::MlModelGroups => "model group",
        DocKind::AdDetectors => "detector",
        DocKind::IsmManaged => "managed index",
        DocKind::NotificationConfigs | DocKind::NotificationCatalogue => "notification config",
        DocKind::IsmTemplates => "template",
        DocKind::ObservabilityObjects => "observability object",
        DocKind::SecurityRoles
        | DocKind::SecurityRolesMapping
        | DocKind::SecurityInternalUsers
        | DocKind::SecurityTenants
        | DocKind::SecurityActionGroups => "security document",
    };
    let refuse = |why: String| {
        // ALLOC: one message, once, for a refused name
        Err(MetaError::InvalidDocName(format!(
            "Invalid {what} name [{name}], {why}"
        )))
    };
    if name.is_empty() {
        // ALLOC: one message, once, for a refused name
        return refuse("must not be empty".to_owned());
    }
    if name.len() > MAX_DOC_NAME_BYTES {
        // ALLOC: one message, once, for a refused name
        return refuse(format!(
            "name is too long: ({} > {MAX_DOC_NAME_BYTES})",
            name.len()
        ));
    }
    if name.chars().any(char::is_control) {
        // ALLOC: one message, once, for a refused name
        return refuse("must not contain control characters".to_owned());
    }
    if let Some(c) = name.chars().find(|c| {
        matches!(
            c,
            '/' | '\\' | '#' | '*' | '?' | '"' | '<' | '>' | '|' | ','
        )
    }) {
        // ALLOC: one message, once, for a refused name
        return refuse(format!("must not contain '{c}'"));
    }
    if name == "." || name == ".." {
        // ALLOC: one message, once, for a refused name
        return refuse("must not be '.' or '..'".to_owned());
    }
    if name.starts_with('_') {
        // ALLOC: one message, once, for a refused name
        return refuse("must not start with '_'".to_owned());
    }
    Ok(())
}
