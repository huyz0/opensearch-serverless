//! `GET|HEAD /{index}/_doc/{id}` and `/{index}/_source/{id}` over the shards
//! this node writes: the realtime read of one document, answered as
//! `RestGetAction` and `RestGetSourceAction` do.

use std::io::Write;
use std::sync::Arc;

use oss_cluster::routing::DocumentRouting;
use oss_rest::{
    Exception, JSON, Length, Meta, Method, Request, Response, RestError, RouteError, Router,
    write_json_str,
};
use oss_writer::bulk::IndexInfo;

use crate::bulk_route::refresh;
use crate::writer::WriterRole;

/// What a get takes.
const DOC_PARAMS: &[&str] = &["realtime", "refresh", "routing", "_source"];

/// What a source get takes.
const SOURCE_PARAMS: &[&str] = &["realtime", "refresh", "routing"];

/// Which of the two APIs answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Api {
    /// The document, with its metadata and `_source`.
    Doc,
    /// Its `_source` alone.
    Source,
}

/// Serves the document get APIs over a [`WriterRole`].
pub struct GetService {
    role: Arc<WriterRole>,
}

/// `IndexNotFoundException` for a name no route knows.
pub(crate) fn index_not_found(index: &str) -> RestError {
    RestError::exception(
        404,
        // ALLOC: error path; one message and the name, three times.
        Exception::new(
            "index_not_found_exception",
            format!("no such index [{index}]"),
        )
        .with_meta("index", Meta::Str(index.to_owned()))
        .with_meta("resource.id", Meta::Str(index.to_owned()))
        .with_meta("resource.type", Meta::Str("index_or_alias".to_owned()))
        .with_meta("index_uuid", Meta::Str("_na_".to_owned())),
    )
}

/// A boolean parameter, `default` when it is absent.
fn flag(req: &Request<'_>, name: &str, default: bool) -> Result<bool, RestError> {
    match req.param(name).as_deref() {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(RestError::illegal_argument(format!(
            "Failed to parse value [{other}] as only [true] or [false] are allowed."
        ))),
    }
}

/// `?_source`: whether the document's source is answered.
fn want_source(req: &Request<'_>) -> Result<bool, RestError> {
    match req.param("_source").as_deref() {
        None | Some("" | "true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(RestError::illegal_argument(format!(
            "_source filtering by [{other}] is not supported"
        ))),
    }
}

/// The shard of `index` that holds `id`.
fn shard_of(index: &IndexInfo, id: &str, routing: Option<&str>) -> Result<u32, RestError> {
    let missing = || {
        RestError::exception(
            400,
            // ALLOC: error path; one message.
            Exception::new(
                "routing_missing_exception",
                format!("routing is required for [{}]/[{id}]", index.name),
            )
            // ALLOC: error path; two names.
            .with_meta("index", Meta::Str(index.name.clone()))
            .with_meta("index_uuid", Meta::Str(index.uuid.clone())),
        )
    };
    if routing.is_none() && index.routing_required {
        return Err(missing());
    }
    DocumentRouting::legacy_scaled(index.shards, index.partition_size, index.routing_shards)
        .and_then(|r| r.shard_for(id, routing))
        .map_err(|_| missing())
}

impl GetService {
    /// A service reading from `role`.
    pub fn new(role: Arc<WriterRole>) -> Self {
        Self { role }
    }

    fn handle(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        api: Api,
    ) -> Result<(), RestError> {
        let name = req.path_param("index").unwrap_or_default();
        let id = req.path_param("id").unwrap_or_default();
        let realtime = flag(req, "realtime", true)?;
        let refresh = refresh(req.param("refresh").as_deref())?;
        let source = api == Api::Source || want_source(req)?;
        let index = self
            .role
            .index_info(&name)
            .ok_or_else(|| index_not_found(&name))?;
        let shard = shard_of(&index, &id, req.param("routing").as_deref())?;
        if refresh {
            self.role
                .refresh_owned(&index, shard)
                .map_err(|e| RestError::simple(500, e))?;
        }
        let found = self
            .role
            .get_owned(&index, shard, &id, realtime)
            .map_err(|reason| {
                RestError::exception(
                    503,
                    Exception::new("no_shard_available_action_exception", reason),
                )
            })?;
        match (api, found) {
            (Api::Doc, found) => {
                let status = if found.is_some() { 200 } else { 404 };
                let mut out = resp.start(status, Some(JSON), Length::Chunked, &[])?;
                out.write_all(b"{\"_index\":")?;
                write_json_str(&mut out, &index.name)?;
                out.write_all(b",\"_id\":")?;
                write_json_str(&mut out, &id)?;
                if let Some(doc) = found {
                    write!(
                        out,
                        ",\"_version\":{},\"_seq_no\":{},\"_primary_term\":{}",
                        doc.version, doc.seq_no, doc.primary_term
                    )?;
                    if let Some(routing) = &doc.routing {
                        out.write_all(b",\"_routing\":")?;
                        write_json_str(&mut out, routing)?;
                    }
                    out.write_all(b",\"found\":true")?;
                    if source {
                        out.write_all(b",\"_source\":")?;
                        out.write_all(&doc.source)?;
                    }
                } else {
                    out.write_all(b",\"found\":false")?;
                }
                out.write_all(b"}")?;
                out.flush()?;
                out.finish()
            }
            (Api::Source, Some(doc)) => {
                let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                out.write_all(&doc.source)?;
                out.flush()?;
                out.finish()
            }
            (Api::Source, None) => Err(RestError::exception(
                404,
                // ALLOC: error path; one message.
                Exception::new(
                    "resource_not_found_exception",
                    format!("Source not found [{}]/[{id}]", index.name),
                ),
            )),
        }
    }
}

/// The routes of the document get APIs: `GET` and `HEAD` of each.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<GetService>) -> Result<Router, RouteError> {
    let mut router = router;
    let table: [(&str, Api, &[&str]); 2] = [
        ("/{index}/_doc/{id}", Api::Doc, DOC_PARAMS),
        ("/{index}/_source/{id}", Api::Source, SOURCE_PARAMS),
    ];
    for (path, api, params) in table {
        for method in [Method::Get, Method::Head] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                params,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle(req, resp, api)
                },
            )?;
        }
    }
    Ok(router)
}
