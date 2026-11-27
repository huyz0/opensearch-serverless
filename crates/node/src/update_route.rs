//! `POST /{index}/_update/{id}` over the shards this node writes: a realtime
//! read, the plan of `oss_coordinator::docs::update`, and one write item that
//! carries the sequence number just read, tried again on a conflict.

use std::io::Write;
use std::sync::Arc;

use oss_coordinator::docs::update::{
    self, Existing, Options, Plan, Refused, Request as Ask, ScriptOp, UNASSIGNED_SEQ_NO,
};
use oss_coordinator::docs::update_script::run_update;
use oss_doc::mapping::json::{Json, parse};
use oss_rest::{
    Exception, JSON, Length, Method, Request, Response, RestError, RouteError, Router,
    write_json_str,
};
use oss_writer::bulk::{Action, Bulk, Item, Kind, Op, doc_status, write_doc};
use oss_writer::index::{Expect, MATCH_ANY, VersionType};

use crate::bulk_route::refresh;
use crate::doc_route::{DocService, now_millis, number};
use crate::get_route::index_not_found;
use crate::id_lanes::IdLanes;
use crate::update_body::{UpdateBody, merge, read};
use crate::writer::{RoleTarget, WriterRole};
use oss_types::stored_scripts::StoredScripts;

/// What an update takes.
const PARAMS: &[&str] = &[
    "refresh",
    "routing",
    "retry_on_conflict",
    "if_seq_no",
    "if_primary_term",
    "require_alias",
];

/// Serves the update API over a [`WriterRole`].
pub struct UpdateService {
    role: Arc<WriterRole>,
    ids: IdLanes,
    scripts: Option<Arc<dyn StoredScripts>>,
}

/// What one attempt decided to write.
enum Step {
    /// Write `source` over the document read, or create it.
    Put { source: Vec<u8>, create: bool },
    /// Delete the document read.
    Delete,
    /// Write nothing.
    Noop,
}

fn refused(r: Refused) -> RestError {
    match r {
        Refused::BadRequest(m) => RestError::illegal_argument(m),
        Refused::NotImplemented(m) => RestError::simple(501, m),
        Refused::Conflict(m) => {
            RestError::exception(409, Exception::new("version_conflict_engine_exception", m))
        }
        Refused::Missing(m) => {
            RestError::exception(404, Exception::new("document_missing_exception", m))
        }
    }
}

fn text(json: &Json) -> Vec<u8> {
    // ALLOC: one document; bounded by the request body and the stored source.
    json.to_json().into_bytes()
}

impl UpdateService {
    /// A service updating through `role`, ids generated from `address`.
    pub fn new(role: Arc<WriterRole>, address: [u8; 6]) -> Self {
        Self {
            role,
            ids: IdLanes::new(address),
            scripts: None,
        }
    }

    /// The same, resolving a script named by `id` through `scripts`.
    #[must_use]
    pub fn with_scripts(mut self, scripts: Arc<dyn StoredScripts>) -> Self {
        self.scripts = Some(scripts);
        self
    }

    /// What the attempt writes, given the document as it was read.
    fn decide(
        body: &UpdateBody,
        plan: Plan,
        index: &str,
        id: &str,
        read: Option<&oss_writer::index::Fetched>,
    ) -> Result<Step, RestError> {
        let script_run = |found: bool, version: i64, doc: &[u8]| {
            let script = body.script.as_ref().ok_or_else(|| {
                RestError::illegal_argument("scripted_upsert was given without a script")
            })?;
            run_update(
                &script.source,
                &script.params,
                found,
                index,
                id,
                version,
                doc,
            )
            .map_err(|reason| {
                RestError::exception(
                    400,
                    // ALLOC: error path; one message.
                    Exception::new("script_exception", reason),
                )
            })
        };
        Ok(match (plan, read) {
            (Plan::Create { from_doc }, _) => {
                let base = if from_doc { &body.doc } else { &body.upsert };
                let base = base.clone().unwrap_or(Json::Obj(Vec::new()));
                if body.scripted_upsert && body.script.is_some() {
                    let out = script_run(false, 0, &text(&base))?;
                    if out.op == ScriptOp::Create {
                        Step::Put {
                            source: out.source,
                            create: true,
                        }
                    } else {
                        Step::Noop
                    }
                } else {
                    Step::Put {
                        source: text(&base),
                        create: true,
                    }
                }
            }
            (Plan::Script, Some(found)) => {
                let out = script_run(true, found.version, &found.source)?;
                match out.op {
                    ScriptOp::Index => Step::Put {
                        source: out.source,
                        create: false,
                    },
                    ScriptOp::Delete => Step::Delete,
                    ScriptOp::Create | ScriptOp::Noop => Step::Noop,
                }
            }
            (Plan::Merge { detect_noop }, Some(found)) => {
                let Ok(current @ Json::Obj(_)) = parse(&found.source) else {
                    return Err(RestError::simple(500, "the stored source is not an object"));
                };
                let mut merged = current.clone();
                if let Some(doc) = &body.doc {
                    merge(&mut merged, doc);
                }
                if detect_noop && merged == current {
                    Step::Noop
                } else {
                    Step::Put {
                        source: text(&merged),
                        create: false,
                    }
                }
            }
            (Plan::Script | Plan::Merge { .. }, None) => {
                return Err(RestError::simple(
                    500,
                    "an update planned over a missing document",
                ));
            }
        })
    }

    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let name = req.path_param("index").unwrap_or_default().into_owned();
        let id = req.path_param("id").unwrap_or_default().into_owned();
        let refresh = refresh(req.param("refresh").as_deref())?;
        let routing = req.param("routing").map(|v| v.into_owned());
        let retry = number(req, "retry_on_conflict")?.unwrap_or(0);
        let if_seq_no = number(req, "if_seq_no")?.unwrap_or(UNASSIGNED_SEQ_NO);
        let if_primary_term = number(req, "if_primary_term")?.unwrap_or(0);
        let require_alias = req.param("require_alias").is_some_and(|v| v == "true");
        let bytes = DocService::source(req)?;
        let body = read(&bytes, self.scripts.as_deref())?;
        let has_script = body.script.is_some();
        if has_script && !oss_coordinator::docs::update_script::enabled() {
            return Err(RestError::simple(
                501,
                "script updates need serverless.scripting.painless.native_compile",
            ));
        }
        update::validate(&Options {
            retry_on_conflict: i32::try_from(retry).unwrap_or(i32::MAX),
            scripted_upsert: body.scripted_upsert,
            has_script,
            require_alias,
            active_shards_above_one: false,
            extra_field_values: false,
        })
        .map_err(refused)?;
        let ask = Ask {
            doc_as_upsert: body.doc_as_upsert,
            has_upsert: body.upsert.is_some(),
            has_script,
            detect_noop: body.detect_noop,
            if_seq_no,
            if_primary_term,
        };
        let index = self
            .role
            .index_info(&name)
            .ok_or_else(|| index_not_found(&name))?;
        let shard = crate::get_route::shard_of(&index, &id, routing.as_deref())?;
        let mut last = None;
        for _ in 0..update::attempts(i32::try_from(retry).unwrap_or(i32::MAX)) {
            let found = self
                .role
                .get_owned(&index, shard, &id, true)
                .map_err(|reason| {
                    RestError::exception(
                        503,
                        Exception::new("no_shard_available_action_exception", reason),
                    )
                })?;
            let existing = Existing {
                found: found.is_some(),
                seq_no: found.as_ref().map_or(0, |f| f.seq_no),
                primary_term: found.as_ref().map_or(0, |f| f.primary_term),
            };
            let plan = update::plan(&name, &id, &existing, &ask).map_err(refused)?;
            let write = Self::decide(&body, plan, &name, &id, found.as_ref())?;
            let cond = update::write_condition(&existing, if_seq_no, if_primary_term);
            let (kind, create, source) = match write {
                Step::Noop => {
                    let f = found.as_ref();
                    let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                    out.write_all(b"{\"_index\":")?;
                    write_json_str(&mut out, &name)?;
                    out.write_all(b",\"_id\":")?;
                    write_json_str(&mut out, &id)?;
                    write!(
                        out,
                        ",\"_version\":{},\"result\":\"noop\",\"_shards\":{{\"total\":0,\"successful\":0,\"failed\":0}},\"_seq_no\":{},\"_primary_term\":{}}}",
                        f.map_or(0, |f| f.version),
                        f.map_or(0, |f| f.seq_no),
                        f.map_or(0, |f| f.primary_term)
                    )?;
                    out.flush()?;
                    return out.finish();
                }
                Step::Delete => (Kind::Delete, false, Vec::new()),
                Step::Put { source, create } => (Kind::Index, create, source),
            };
            let action = Action {
                kind,
                index: Some(name.clone()),
                id: Some(id.clone()),
                routing: routing.clone(),
                create: create || cond.require_absent,
                expect: Expect {
                    version: MATCH_ANY,
                    version_type: VersionType::Internal,
                    if_seq_no: cond.if_seq_no,
                    if_primary_term: cond.if_primary_term,
                },
                pipeline: None,
                require_alias: false,
            };
            let mut target = RoleTarget::new(&self.role);
            let mut ids = self.ids.generator();
            let mut run = Bulk::new(&mut target, &mut ids, now_millis(), refresh);
            run.slice(&[Op { action, source }])?;
            let mut items = run.finish()?;
            let Some(item) = items.pop() else {
                return Err(RestError::simple(500, "the write produced no result"));
            };
            if doc_status(&item) == 409 {
                last = Some(item);
                continue;
            }
            return answer(resp, &item);
        }
        match last {
            Some(item) => answer(resp, &item),
            None => Err(RestError::simple(500, "the update made no attempt")),
        }
    }
}

fn answer(resp: &mut Response<'_>, item: &Item) -> Result<(), RestError> {
    let mut out = resp.start(doc_status(item), Some(JSON), Length::Chunked, &[])?;
    write_doc(&mut out, item)?;
    out.flush()?;
    out.finish()
}

/// The route of the update API.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<UpdateService>) -> Result<Router, RouteError> {
    router.route(
        Method::Post,
        "/{index}/_update/{id}",
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
    )
}
