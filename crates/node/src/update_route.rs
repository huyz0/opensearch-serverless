//! `POST /{index}/_update/{id}` over the shards this node writes: a realtime
//! read, the plan of `oss_coordinator::docs::update`, and one write item that
//! carries the sequence number just read, tried again on a conflict.

use std::io::Write;
use std::sync::Arc;

use oss_coordinator::docs::update::{self, UNASSIGNED_SEQ_NO};
use oss_rest::{
    JSON, Length, Method, Request, Response, RestError, RouteError, Router, write_json_str,
};
use oss_writer::bulk::{Action, Bulk, Item, Kind, Op, Resolved, doc_status, write_doc};
use oss_writer::index::{Expect, MATCH_ANY, VersionType};

use crate::bulk_route::refresh;
use crate::doc_route::{DocService, now_millis, number};
use crate::get_route::index_not_found;
use crate::id_lanes::IdLanes;
use crate::update_body::read;
use crate::update_decide::{Failure, check, resolve_once};
use crate::writer::{RoleTarget, WriterRole};

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
}

fn rest(f: Failure) -> RestError {
    RestError::exception(f.status, f.error)
}

impl UpdateService {
    /// A service updating through `role`, ids generated from `address`.
    pub fn new(role: Arc<WriterRole>, address: [u8; 6]) -> Self {
        Self {
            role,
            ids: IdLanes::new(address),
        }
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
        let body = read(&bytes).map_err(rest)?;
        let retry = i32::try_from(retry).unwrap_or(i32::MAX);
        check(&body, retry, require_alias).map_err(rest)?;
        let index = self
            .role
            .index_info(&name)
            .ok_or_else(|| index_not_found(&name))?;
        let shard = crate::get_route::shard_of(&index, &id, routing.as_deref())?;
        let mut last = None;
        for _ in 0..update::attempts(retry) {
            let resolved = resolve_once(
                &self.role,
                &index,
                shard,
                &id,
                &body,
                if_seq_no,
                if_primary_term,
            )
            .map_err(rest)?;
            let (kind, create, source, if_seq_no, if_primary_term) = match resolved {
                Resolved::Noop {
                    version,
                    seq_no,
                    primary_term,
                } => {
                    let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                    out.write_all(b"{\"_index\":")?;
                    write_json_str(&mut out, &name)?;
                    out.write_all(b",\"_id\":")?;
                    write_json_str(&mut out, &id)?;
                    write!(
                        out,
                        ",\"_version\":{version},\"result\":\"noop\",\"_shards\":{{\"total\":0,\"successful\":0,\"failed\":0}},\"_seq_no\":{seq_no},\"_primary_term\":{primary_term}}}"
                    )?;
                    out.flush()?;
                    return out.finish();
                }
                Resolved::Delete {
                    if_seq_no,
                    if_primary_term,
                } => (Kind::Delete, false, Vec::new(), if_seq_no, if_primary_term),
                Resolved::Put {
                    source,
                    create,
                    if_seq_no,
                    if_primary_term,
                } => (Kind::Index, create, source, if_seq_no, if_primary_term),
            };
            let action = Action {
                kind,
                index: Some(name.clone()),
                id: Some(id.clone()),
                routing: routing.clone(),
                create,
                expect: Expect {
                    version: MATCH_ANY,
                    version_type: VersionType::Internal,
                    if_seq_no,
                    if_primary_term,
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
