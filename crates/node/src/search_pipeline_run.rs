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

/// `index.search.default_pipeline`.
const DEFAULT_KEY: &str = "index.search.default_pipeline";

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

    /// The pipeline an index's settings name for a search that names none.
    fn default_pipeline(&self, index: &str) -> Result<Option<String>, Answer> {
        // The search opens this shard next and would fail the same way, so a
        // failure ends it here rather than being paid for twice.
        let held = self.open_shard(index, 0, false)?;
        Ok(match held.shard.meta.settings.get(DEFAULT_KEY) {
            Some(oss_format::Setting::Value(id)) => Some(id.clone()),
            _ => None,
        })
    }

    /// The pipeline a search runs: what `?search_pipeline=` names, else the
    /// body's, else the default of `index`. `None` when none, or `_none`.
    fn pipeline_id(
        &self,
        param: Option<String>,
        source: &SearchSource,
        index: Option<&str>,
    ) -> Result<Option<String>, Answer> {
        let id = match (param, source.search_pipeline.as_ref()) {
            (Some(id), _) => id,
            (None, Some(id)) => id.clone(),
            (None, None) => match (self.pipelines.get(), index) {
                (Some(_), Some(index)) if source.pit.is_none() => {
                    match self.default_pipeline(index)? {
                        Some(id) => id,
                        None => return Ok(None),
                    }
                }
                _ => return Ok(None),
            },
        };
        Ok((!id.is_empty() && id != "_none").then_some(id))
    }

    /// Runs the pipeline `?search_pipeline=` names over `source`, or when it
    /// names none the body's, or the default of `index`; nothing when it is
    /// `_none`.
    pub(super) fn run_pipeline(
        &self,
        req: &mut Request<'_>,
        source: &mut SearchSource,
        index: Option<&str>,
    ) -> Result<(), Answer> {
        let param = req.param("search_pipeline").map(|id| id.into_owned());
        let Some(id) = self.pipeline_id(param, source, index)? else {
            return Ok(());
        };
        let pool = req.body();
        self.apply_pipeline(&id, source, |n| pool.reserve(n).map_err(Answer::Rest))
    }

    /// Runs the pipeline an `_msearch` item names (its body's, else its
    /// index's default) over `source`. The pipeline's buffers are held by the
    /// returned reservation until the item is answered.
    pub(super) fn run_item_pipeline(
        &self,
        index: &str,
        source: &mut SearchSource,
    ) -> Result<Option<oss_mem::Reservation>, Answer> {
        let Some(id) = self.pipeline_id(None, source, Some(index))? else {
            return Ok(None);
        };
        let mut held = None;
        self.apply_pipeline(&id, source, |n| {
            self.budget
                .try_reserve(oss_mem::Pool::RequestBodies, n)
                .map(|g| held = Some(g))
                .map_err(|e| Answer::Rest(RestError::circuit_breaking(&e, "search_pipeline")))
        })?;
        Ok(held)
    }

    /// Resolves pipeline `id` after `hold` reserved its buffers, and applies
    /// it to `source`.
    fn apply_pipeline(
        &self,
        id: &str,
        source: &mut SearchSource,
        hold: impl FnOnce(usize) -> Result<(), Answer>,
    ) -> Result<(), Answer> {
        let Some(pipelines) = self.pipelines.get() else {
            return Err(refuse(
                501,
                "not_implemented",
                "search pipelines are not enabled on this node (OSS_SEARCH_PIPELINES)",
            ));
        };
        hold(pipelines.reserve_bytes())?;
        let pipeline = pipelines.resolve(id).map_err(|fault| match fault {
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
