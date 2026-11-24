//! The pipelines a bulk runs over its items: what the node plugs in, since
//! the writer cannot depend on the coordinator that holds the engine.

/// Why a pipeline did not produce a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestError {
    /// No pipeline has this id.
    Missing,
    /// A processor failed the document; the text is its message.
    Failed(String),
    /// The pipelines could not be read; the text says why.
    Unavailable(String),
}

/// Runs ingest pipelines for one bulk. An implementation is made for the
/// request and may hold what it read (a snapshot of the pipelines) until the
/// request ends, so a bulk reads the pipeline store once, not once an item.
pub trait Ingest {
    /// Runs pipeline `id` over `source` and answers the document to index.
    ///
    /// # Errors
    ///
    /// [`IngestError::Missing`] for an unknown pipeline, the failure of a
    /// processor, or [`IngestError::Unavailable`] when the pipelines cannot
    /// be read; each fails that item alone.
    fn run(&mut self, id: &str, source: &[u8]) -> Result<Vec<u8>, IngestError>;
}
