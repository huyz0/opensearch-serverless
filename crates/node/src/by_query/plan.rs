//! A by-query or reindex request ready to run: validated, owning what it
//! needs, run inline or as a background task.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use oss_coordinator::docs::by_query::{Walk, walk};
use oss_rest::{JSON, Request, Response, RestError};
use oss_writer::bulk::{IdGenerator, Target};

use super::change::{DeleteChange, UpdateChange};
use super::params::{Mode, Params};
use super::reindex::ReindexChange;
use super::tasks::Slot;
use super::{ByQueryService, QuerySource, missing, now_millis, response};
use crate::writer::RoleTarget;

const DELETE_ACTION: &str = "indices:data/write/delete/byquery";
const UPDATE_ACTION: &str = "indices:data/write/update/byquery";
const REINDEX_ACTION: &str = "indices:data/write/reindex";

/// The run of a request: its status and body, given the cancel flag.
type Job = Box<dyn FnOnce(&AtomicBool) -> Result<(u16, String), RestError> + Send>;

/// A validated request.
pub(super) struct Plan {
    action: &'static str,
    description: String,
    background: bool,
    job: Job,
}

fn took(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn script_off() -> RestError {
    RestError::simple(
        501,
        "script updates need serverless.scripting.painless.native_compile",
    )
}

impl ByQueryService {
    /// `POST /{index}/_delete_by_query`, validated.
    pub(super) fn plan_delete(self: &Arc<Self>, req: &mut Request<'_>) -> Result<Plan, RestError> {
        let params = Params::read(req, Mode::Delete, self.scripts.as_deref())?;
        let index = index_of(req)?;
        let info = RoleTarget::new(&self.role)
            .index(&index)
            .ok_or_else(|| missing(&index))?;
        let me = Arc::clone(self);
        Ok(Plan {
            action: DELETE_ACTION,
            description: format!("delete-by-query [{index}]"), // ALLOC: one description.
            background: params.background,
            job: Box::new(move |cancel| {
                let started = Instant::now();
                let shards: Vec<u32> = (0..info.shards).collect();
                let source = QuerySource {
                    matches: &*me.matches,
                    budget: &me.budget,
                    index: &index,
                    query: &params.query,
                    cancel,
                };
                let mut ids = IdGenerator::new(0, me.address);
                let mut change = DeleteChange::new(&me.role, &info, &mut ids, now_millis());
                let outcome: Walk = walk(
                    &source,
                    &shards,
                    params.max_docs,
                    params.refresh,
                    params.abort_on_conflict,
                    &mut change,
                )?;
                Ok(response::mark_cancelled(
                    cancel,
                    response::render(&outcome, &index, took(started)),
                ))
            }),
        })
    }

    /// `POST /{index}/_update_by_query`, validated.
    pub(super) fn plan_update(self: &Arc<Self>, req: &mut Request<'_>) -> Result<Plan, RestError> {
        if !oss_coordinator::docs::update_script::enabled() {
            return Err(script_off());
        }
        let params = Params::read(req, Mode::Update, self.scripts.as_deref())?;
        if params.script.is_none() {
            return Err(RestError::illegal_argument("[script] is required"));
        }
        let index = index_of(req)?;
        let info = RoleTarget::new(&self.role)
            .index(&index)
            .ok_or_else(|| missing(&index))?;
        let me = Arc::clone(self);
        Ok(Plan {
            action: UPDATE_ACTION,
            description: format!("update-by-query [{index}]"), // ALLOC: one description.
            background: params.background,
            job: Box::new(move |cancel| {
                let started = Instant::now();
                let Some(script) = params.script.as_ref() else {
                    return Err(RestError::illegal_argument("[script] is required"));
                };
                let shards: Vec<u32> = (0..info.shards).collect();
                let source = QuerySource {
                    matches: &*me.matches,
                    budget: &me.budget,
                    index: &index,
                    query: &params.query,
                    cancel,
                };
                let mut ids = IdGenerator::new(0, me.address);
                let mut change = UpdateChange::new(&me.role, &info, script, &mut ids, now_millis());
                let outcome: Walk = walk(
                    &source,
                    &shards,
                    params.max_docs,
                    params.refresh,
                    params.abort_on_conflict,
                    &mut change,
                )?;
                let counts = (change.updated, change.deleted);
                Ok(response::mark_cancelled(
                    cancel,
                    response::render_update(&outcome, &index, took(started), counts),
                ))
            }),
        })
    }

    /// `POST /_reindex`, validated.
    pub(super) fn plan_reindex(self: &Arc<Self>, req: &mut Request<'_>) -> Result<Plan, RestError> {
        let params = Params::read(req, Mode::Reindex, self.scripts.as_deref())?;
        let Some(target) = params.reindex.as_ref() else {
            return Err(RestError::illegal_argument("a reindex names a source"));
        };
        if params.script.is_some() && !oss_coordinator::docs::update_script::enabled() {
            return Err(script_off());
        }
        if target.source == target.dest {
            return Err(RestError::illegal_argument(
                "reindex cannot write into an index its reading from",
            ));
        }
        let mut target_role = RoleTarget::new(&self.role);
        let source_info = target_role
            .index(&target.source)
            .ok_or_else(|| missing(&target.source))?;
        let dest_info = target_role
            .index(&target.dest)
            .ok_or_else(|| missing(&target.dest))?;
        let description = format!(
            "reindex from [{}] to [{}][_doc]",
            target.source, target.dest
        ); // ALLOC: one description.
        let me = Arc::clone(self);
        Ok(Plan {
            action: REINDEX_ACTION,
            description,
            background: params.background,
            job: Box::new(move |cancel| {
                let started = Instant::now();
                let Some(target) = params.reindex.as_ref() else {
                    return Err(RestError::illegal_argument("a reindex names a source"));
                };
                let shards: Vec<u32> = (0..source_info.shards).collect();
                let source = QuerySource {
                    matches: &*me.matches,
                    budget: &me.budget,
                    index: &target.source,
                    query: &params.query,
                    cancel,
                };
                let mut ids = IdGenerator::new(0, me.address);
                let mut change = ReindexChange::new(
                    &me.role,
                    (&source_info, &dest_info),
                    target.create,
                    params.script.as_ref(),
                    &mut ids,
                    now_millis(),
                );
                let outcome: Walk = walk(
                    &source,
                    &shards,
                    params.max_docs,
                    params.refresh,
                    params.abort_on_conflict,
                    &mut change,
                )?;
                let counts = (change.created, change.updated, change.deleted);
                Ok(response::mark_cancelled(
                    cancel,
                    response::render_reindex(&outcome, &target.dest, took(started), counts),
                ))
            }),
        })
    }

    /// Runs `plan` and answers: inline, or as a background task whose id is
    /// the answer.
    pub(super) fn serve(
        self: &Arc<Self>,
        plan: Plan,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        if !plan.background {
            let (status, body) = (plan.job)(&AtomicBool::new(false))?;
            return resp.send(status, JSON, body.as_bytes());
        }
        let (id, cancel) = self
            .tasks
            .start(plan.action, &plan.description, now_millis())?;
        let slot = Slot::new(Arc::clone(&self.tasks), id, now_millis);
        let job = plan.job;
        std::thread::Builder::new()
            .name(String::from("oss-task")) // ALLOC: one name a task.
            .spawn(move || {
                let result = job(&cancel).map(|(_status, body)| body);
                slot.finish(result);
            })
            .map_err(|_| RestError::simple(500, "a task thread could not start"))?;
        // ALLOC: one small body a request.
        let body = format!(r#"{{"task":"{}:{id}"}}"#, self.tasks.node());
        resp.send(200, JSON, body.as_bytes())
    }
}

fn index_of(req: &Request<'_>) -> Result<String, RestError> {
    Ok(req
        .path_param("index")
        .ok_or_else(|| RestError::illegal_argument("an index is required"))?
        .into_owned())
}
