//! The route table of asynchronous search, on `/_plugins/_asynchronous_search`
//! and the legacy `/_opendistro/_asynchronous_search`.

use std::sync::Arc;

use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use super::{Reply, Service, Submit};
use crate::jobs::Who;

/// The largest search body a submit takes.
const MAX_BODY_BYTES: usize = 1 << 20;

/// The query parameters a request accepts; the service refuses the unknown
/// ones itself, with the stock plugin's message.
const PARAMS: &[&str] = &[
    "wait_for_completion_timeout",
    "keep_on_completion",
    "keep_alive",
    "batched_reduce_size",
    "request_cache",
    "routing",
    "preference",
    "allow_no_indices",
    "expand_wildcards",
    "ignore_unavailable",
    "typed_keys",
    "rest_total_hits_as_int",
];

fn query_of(req: &Request<'_>) -> Vec<(String, String)> {
    req.raw_query()
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|pair| {
            let (name, _) = pair.split_once('=').unwrap_or((pair, ""));
            // ALLOC: at most the parameters of one request.
            req.param(name).map(|v| (name.to_owned(), v.into_owned()))
        })
        .collect()
}

/// Who a request acts as. A get or delete names no index, so a tenant read
/// from the path would not match the one that submitted: every request is of
/// the one tenant, and the user (the authenticated name) is what isolates.
fn who_of(req: &Request<'_>) -> Who {
    Who {
        tenant: "_".to_owned(), // ALLOC: one name per request.
        user: req.principal().unwrap_or_default().to_owned(), // ALLOC: one name per request.
    }
}

fn read_body(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
    let body = req.body();
    // ALLOC: the body so far, at most `MAX_BODY_BYTES`, reserved below.
    let mut out: Vec<u8> = Vec::new();
    loop {
        let chunk = body.max_chunk();
        let more = out
            .len()
            .saturating_add(chunk)
            .saturating_sub(body.reserved());
        body.reserve(more)?;
        let Some(next) = body.next_chunk()? else {
            break;
        };
        if out.len().saturating_add(next.len()) > MAX_BODY_BYTES {
            return Err(RestError::too_large());
        }
        out.extend_from_slice(next);
    }
    Ok(out)
}

fn send(resp: &mut Response<'_>, reply: &Reply) -> Result<(), RestError> {
    resp.send(reply.status, JSON, &reply.body)
}

fn path_of(req: &Request<'_>) -> String {
    req.raw_path().to_owned() // ALLOC: one path per request, for the messages that name it.
}

fn submit(
    service: &Service,
    req: &mut Request<'_>,
    resp: &mut Response<'_>,
) -> Result<(), RestError> {
    let index = req.path_param("index");
    let who = who_of(req);
    let query = query_of(req);
    let path = path_of(req);
    let body = read_body(req)?;
    let reply = service.submit(&Submit {
        index: index.as_deref(),
        path: &path,
        query: &query,
        body: &body,
        who: &who,
    })?;
    send(resp, &reply)
}

fn get(service: &Service, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let id = req.path_param("id").unwrap_or_default().into_owned();
    let who = who_of(req);
    let reply = service.get(&who, &id, &path_of(req), &query_of(req))?;
    send(resp, &reply)
}

fn delete(
    service: &Service,
    req: &mut Request<'_>,
    resp: &mut Response<'_>,
) -> Result<(), RestError> {
    let id = req.path_param("id").unwrap_or_default().into_owned();
    let reply = service.delete(&who_of(req), &id)?;
    send(resp, &reply)
}

fn stats(
    service: &Service,
    _req: &mut Request<'_>,
    resp: &mut Response<'_>,
) -> Result<(), RestError> {
    send(resp, &service.stats())
}

type Handler = fn(&Service, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The routes: submit by POST with or without an index, get and delete by id,
/// and this node's stats.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<Service>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, Handler); 4] = [
        (Method::Post, "", submit),
        (Method::Get, "/stats", stats),
        (Method::Get, "/{id}", get),
        (Method::Delete, "/{id}", delete),
    ];
    let mut router = router;
    for root in [
        "/_plugins/_asynchronous_search",
        "/_opendistro/_asynchronous_search",
    ] {
        for (method, tail, handler) in table {
            router = add(router, service, method, format!("{root}{tail}"), handler)?;
        }
        router = add(
            router,
            service,
            Method::Post,
            format!("/{{index}}{root}"),
            submit,
        )?;
    }
    Ok(router)
}

fn add(
    router: Router,
    service: &Arc<Service>,
    method: Method,
    path: String,
    handler: Handler,
) -> Result<Router, RouteError> {
    // ALLOC: the router keeps its patterns for the life of the process.
    let path: &'static str = Box::leak(path.into_boxed_str());
    let service = Arc::clone(service);
    router.route(
        method,
        path,
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| handler(&service, req, resp),
    )
}
