//! Running a stored search pipeline over a `_search`: the seam the node's
//! pipeline set plugs into, and what a search does with the answer.
//!
//! A search that names no pipeline never reaches this module's work: the
//! parameter is read, found absent, and nothing else happens.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_reader::dsl::SearchSource;
use oss_reader::search_pipeline::SearchPipeline;
use oss_rest::{Exception, Request, RestError};

use super::{Answer, SearchService};

/// Why a stored pipeline could not be had.
#[derive(Debug)]
pub enum PipelineFault {
    /// No pipeline is stored under the id.
    Missing,
    /// The set could not be read.
    Unreadable(String),
    /// The stored definition no longer compiles.
    Broken(String),
}

/// Where a search finds the pipelines it names.
pub trait PipelineSource: Send + Sync {
    /// The bytes a search reserves from the request-body pool before it
    /// asks for a pipeline: what reading the set holds.
    fn reserve_bytes(&self) -> usize;

    /// The pipeline stored under `id`.
    ///
    /// # Errors
    ///
    /// [`PipelineFault`] for an id nothing is stored under, an unreadable
    /// set, or a definition that does not compile.
    fn resolve(&self, id: &str) -> Result<SearchPipeline, PipelineFault>;
}

fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> Answer {
    Answer::Rest(RestError::exception(status, Exception::new(kind, reason)))
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Installs where `?search_pipeline=` finds its pipelines. False when
    /// one is installed already.
    pub fn set_search_pipelines(&self, source: Arc<dyn PipelineSource>) -> bool {
        self.pipelines.set(source).is_ok()
    }

    /// Runs the pipeline `?search_pipeline=` names over `source`; nothing
    /// when it names none or `_none`.
    pub(super) fn run_pipeline(
        &self,
        req: &mut Request<'_>,
        source: &mut SearchSource,
    ) -> Result<(), Answer> {
        let Some(id) = req.param("search_pipeline") else {
            return Ok(());
        };
        if id.is_empty() || id == "_none" {
            return Ok(());
        }
        let Some(pipelines) = self.pipelines.get() else {
            return Err(refuse(
                501,
                "not_implemented",
                "search pipelines are not enabled on this node (OSS_SEARCH_PIPELINES)",
            ));
        };
        req.body()
            .reserve(pipelines.reserve_bytes())
            .map_err(Answer::Rest)?;
        let pipeline = pipelines.resolve(&id).map_err(|fault| match fault {
            // ALLOC: error path; one message.
            PipelineFault::Missing => refuse(
                400,
                "pipeline_missing",
                format!("no such search pipeline: {id}"),
            ),
            // ALLOC: error path; one message.
            PipelineFault::Unreadable(e) => refuse(
                500,
                "search_pipeline_processing_exception",
                format!("search pipelines could not be read: {e}"),
            ),
            PipelineFault::Broken(e) => refuse(500, "search_pipeline_processing_exception", e),
        })?;
        pipeline.apply(source).map_err(|e| {
            refuse(
                500,
                "search_pipeline_processing_exception",
                // ALLOC: error path; one message.
                e.to_string(),
            )
        })
    }
}
