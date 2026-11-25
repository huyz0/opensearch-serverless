//! The analyzer modules a reader node loads for an index that pins them
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)).

use std::sync::Arc;

use oss_ext::pin::{Pin, ResolvedLimits};
use oss_reader::pinned::PinnedTokens;

/// What loads an index's pinned modules for a search node. A node with none
/// refuses to open an index that pins, and never serves it with the
/// standard analyzer.
pub trait ModuleOpener: Send + Sync {
    /// The tokens of the `pins` of `index`, each distinct module read once
    /// and held for as long as the returned value lives.
    ///
    /// # Errors
    ///
    /// The words of the refusal, starting `extension_unavailable`.
    fn open(
        &self,
        index: &str,
        pins: Vec<(Pin, ResolvedLimits)>,
    ) -> Result<Arc<dyn PinnedTokens>, String>;
}

/// Lets `search` open an index that pins analyzers, when the node is built
/// with extensions and has a pool for them; otherwise it refuses such an
/// index.
pub fn install<S: oss_blob::BlobStore + 'static>(
    search: &crate::search::SearchService<S>,
    store: &Arc<dyn oss_blob::BlobStore>,
    base: &str,
    budget: &oss_mem::MemoryBudget,
) {
    #[cfg(feature = "extensions")]
    if budget.limit(oss_mem::Pool::Extensions) > 0 {
        // A search never replays, so the factor is not read.
        let source = crate::extension_modules::ModuleSource::new(
            Arc::clone(store),
            base,
            crate::extension_host::host(),
            budget.clone(),
            1,
        );
        let _ = search.set_modules(Arc::new(source));
    }
    let _ = (search, store, base, budget);
}
