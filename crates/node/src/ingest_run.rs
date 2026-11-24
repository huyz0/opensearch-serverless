//! The pipelines a bulk or a document write runs, read from the ingest
//! pipeline set.
//!
//! A request makes one [`Run`]; the first pipeline it names reads the set
//! through the node's snapshot cache (one GET of the marker when the set is
//! unchanged), and every later item of the request reuses that snapshot and
//! the pipelines compiled from it, so a bulk costs one read, not one an item.

use std::collections::HashMap;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::ingest::Pipeline;
use oss_format::DocKind;
use oss_meta::{DocSet, DocSetCache, DocSnapshot, MetaError};
use oss_writer::bulk::{Ingest, IngestError};

/// The ingest pipeline set and what the node keeps of it.
pub struct IngestPipelines {
    docs: DocSet,
    cache: DocSetCache,
    buffer_bytes: usize,
}

impl IngestPipelines {
    /// The set under `base` in `store`; a run reads it through two buffers
    /// of `buffer_bytes`, which the caller reserves ([`Self::reserve_bytes`]).
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn open(
        store: &Arc<dyn BlobStore>,
        base: &str,
        buffer_bytes: usize,
    ) -> Result<Arc<Self>, MetaError> {
        // ALLOC: one holder, once, at boot.
        Ok(Arc::new(Self {
            docs: DocSet::new(Arc::clone(store), base, DocKind::IngestPipeline)?,
            cache: DocSetCache::new(),
            buffer_bytes,
        }))
    }

    /// What a run holds while it reads: its two buffers.
    pub fn reserve_bytes(&self) -> usize {
        self.buffer_bytes.saturating_mul(2)
    }

    /// A run for one request.
    pub fn run(&self) -> Run<'_> {
        Run {
            set: self,
            snapshot: None,
            // ALLOC: one entry a pipeline the request names.
            compiled: HashMap::new(),
        }
    }
}

/// One request's pipelines.
pub struct Run<'a> {
    set: &'a IngestPipelines,
    snapshot: Option<Arc<DocSnapshot>>,
    compiled: HashMap<String, Option<Result<Pipeline, String>>>,
}

impl Run<'_> {
    fn snapshot(&mut self) -> Result<Arc<DocSnapshot>, MetaError> {
        if let Some(held) = &self.snapshot {
            return Ok(Arc::clone(held));
        }
        // ALLOC: two buffers a request that names a pipeline, reserved by the caller.
        let mut marker = vec![0u8; self.set.buffer_bytes];
        // ALLOC: as above.
        let mut body = vec![0u8; self.set.buffer_bytes];
        let read = self
            .set
            .docs
            .snapshot(&self.set.cache, &mut marker, &mut body)?;
        self.snapshot = Some(Arc::clone(&read));
        Ok(read)
    }
}

impl Ingest for Run<'_> {
    fn run(&mut self, id: &str, source: &[u8]) -> Result<Vec<u8>, IngestError> {
        if !self.compiled.contains_key(id) {
            let snapshot = self.snapshot().map_err(|e| {
                // ALLOC: error path; one message.
                IngestError::Unavailable(format!("ingest pipelines could not be read: {e}"))
            })?;
            let made = snapshot.get(id).map(Pipeline::compile);
            // ALLOC: one entry a pipeline the request names.
            self.compiled.insert(id.to_owned(), made);
        }
        match self.compiled.get(id) {
            Some(Some(Ok(pipeline))) => pipeline.run(source).map_err(IngestError::Failed),
            Some(Some(Err(reason))) => Err(IngestError::Failed(reason.clone())),
            _ => Err(IngestError::Missing),
        }
    }
}
