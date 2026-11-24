//! The templates a create request inherits from.
//!
//! Reads the index and component template sets through the node-local
//! snapshot cache, resolves the template that wins the new index's name, and
//! layers it under the request (`oss_coordinator::indices::create_inherit`).

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::indices::create::CreateRequest;
use oss_coordinator::indices::create_inherit::inherit;
use oss_coordinator::route::resolve::Refusal;
use oss_coordinator::templates::resolve;
use oss_format::DocKind;
use oss_meta::{DocSet, DocSetCache, MetaError};

/// The two template sets and what the node keeps of them.
pub struct TemplateSets {
    index: DocSet,
    component: DocSet,
    index_cache: DocSetCache,
    component_cache: DocSetCache,
}

impl TemplateSets {
    /// Sets read through `index` and `component`, each cached on this node.
    pub fn new(index: DocSet, component: DocSet) -> Arc<Self> {
        // ALLOC: one holder, once, at boot.
        Arc::new(Self {
            index,
            component,
            index_cache: DocSetCache::new(),
            component_cache: DocSetCache::new(),
        })
    }

    /// The sets of the deployment at `base` in `store`.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn open(store: &Arc<dyn BlobStore>, base: &str) -> Result<Arc<Self>, MetaError> {
        Ok(Self::new(
            DocSet::new(Arc::clone(store), base, DocKind::IndexTemplate)?,
            DocSet::new(Arc::clone(store), base, DocKind::ComponentTemplate)?,
        ))
    }

    /// `request` with the template that matches `index` layered under it.
    ///
    /// One GET of the index template marker when the set is unchanged and
    /// nothing matches; the component marker is read only when a template
    /// is stored. `marker` and `body` are scratch buffers.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] for an unreadable store or template, or one `inherit`
    /// refuses.
    pub fn apply(
        &self,
        index: &str,
        request: CreateRequest,
        marker: &mut [u8],
        body: &mut [u8],
    ) -> Result<CreateRequest, Refusal> {
        let templates = self
            .index
            .snapshot(&self.index_cache, marker, body)
            .map_err(failed)?;
        let mut docs = templates.docs().peekable();
        if docs.peek().is_none() {
            return Ok(request);
        }
        let components = self
            .component
            .snapshot(&self.component_cache, marker, body)
            .map_err(failed)?;
        let inherited = resolve(docs, |name| components.get(name), index)
            .map_err(|e| Refusal::new(400, "invalid_index_template_exception", e.to_string()))?;
        inherit(request, &inherited)
    }
}

fn failed(e: MetaError) -> Refusal {
    // ALLOC: error path; one message.
    Refusal::new(500, "index_create_failure", format!("templates: {e}"))
}
