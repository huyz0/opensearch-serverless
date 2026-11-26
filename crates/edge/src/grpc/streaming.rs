//! `SearchService/ServerStreamSearch`: one `SearchResponse` message per page of hits.
//!
//! Pages are fetched with `search_after` on the caller's sort, one REST `_search` at a time, and
//! each is written before the next is asked for, so a slow reader slows the loopback and the
//! front holds one page. Each fetch asks for one hit more than the page: if that hit sorts equal
//! to the page's last, `search_after` would skip the hits that tie with it, so the stream ends
//! with `FAILED_PRECONDITION` instead of dropping them silently.

use std::time::Duration;

use bytes::Bytes;
use h2::Reason;
use h2::SendStream;
use h2::server::SendResponse;
use http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::{Value as Json, json};
use tokio::time::Instant;

use super::rest::{Rest, encode, fetch};
use super::search::StreamPlan;
use super::status::{DEADLINE_EXCEEDED, FAILED_PRECONDITION, INTERNAL, Status};
use super::{frame, schema, status, trailers_only};
use crate::stream::{Shared, send_all};

/// Runs `work` until the call's deadline, if it has one.
async fn within<T>(
    deadline: Option<Instant>,
    work: impl std::future::Future<Output = T>,
) -> Result<T, Status> {
    match deadline {
        None => Ok(work.await),
        Some(at) => tokio::time::timeout_at(at, work)
            .await
            .map_err(|_| Status::new(DEADLINE_EXCEEDED, "deadline exceeded")),
    }
}

/// The page after `after`, one hit longer than `page`, as a REST request.
fn page_request(plan: &StreamPlan, after: Option<&Json>) -> Result<Rest, Status> {
    let mut body = plan.body.clone();
    // ALLOC: one page of hits, within `grpc_max_message` and charged to the request-body pool at start.
    body.insert("size".to_owned(), json!(plan.page.saturating_add(1)));
    if let Some(after) = after {
        // ALLOC: one page of hits, within `grpc_max_message` and charged to the request-body pool at start.
        body.insert("search_after".to_owned(), after.clone());
    }
    // ALLOC: one request body, within `grpc_max_message` and charged to the request-body pool at start.
    let body = serde_json::to_vec(&Json::Object(body))
        .map_err(|_| Status::new(INTERNAL, "a page request does not encode"))?;
    Ok(Rest {
        method: "POST",
        target: plan.target.clone(),
        body,
    })
}

/// The sort values of a hit.
fn sort_of(hit: &Json) -> Result<&Json, Status> {
    hit.get("sort")
        .ok_or_else(|| Status::new(INTERNAL, "a hit in the answer has no sort values"))
}

/// What one fetch gives: the page's answer, cut to the page size, and where the next starts.
struct Page {
    answer: Json,
    next: Option<Json>,
}

async fn next_page(
    shared: &Shared,
    plan: &StreamPlan,
    after: Option<&Json>,
) -> Result<Page, Status> {
    let mut answer = fetch(shared, &page_request(plan, after)?).await?;
    let hits = answer
        .get_mut("hits")
        .and_then(|h| h.get_mut("hits"))
        .and_then(Json::as_array_mut)
        .ok_or_else(|| Status::new(INTERNAL, "the answer has no hits array"))?;
    if hits.len() <= plan.page {
        return Ok(Page { answer, next: None });
    }
    let lookahead = hits.pop().unwrap_or_default();
    let last = hits
        .last()
        .map(sort_of)
        .transpose()?
        .cloned()
        .unwrap_or_default();
    if *sort_of(&lookahead)? == last {
        return Err(Status::new(
            FAILED_PRECONDITION,
            "the sort does not tell the hit after this page from the page's last hit, so paging would skip hits: sort on a field that is unique, as the last sort key",
        ));
    }
    Ok(Page {
        answer,
        next: Some(last),
    })
}

fn headers_sent(respond: &mut SendResponse<Bytes>) -> Option<SendStream<Bytes>> {
    let response = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/grpc")
        .body(())
        .ok()?;
    respond.send_response(response, false).ok()
}

fn end(send: &mut SendStream<Bytes>, status: &Status) {
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from(u16::from(status.code)));
    if !status.message.is_empty()
        && let Ok(v) = HeaderValue::from_str(&status::encode_message(&status.message))
    {
        trailers.insert("grpc-message", v);
    }
    if send.send_trailers(trailers).is_err() {
        send.send_reset(Reason::INTERNAL_ERROR);
    }
}

/// Serves the stream to its end: a message a page, then the status.
pub(crate) async fn serve(
    shared: &Shared,
    respond: &mut SendResponse<Bytes>,
    plan: &StreamPlan,
    limit: Option<Duration>,
) {
    let deadline = limit.and_then(|d| Instant::now().checked_add(d));
    let response = schema::message("SearchResponse");
    let mut send: Option<SendStream<Bytes>> = None;
    let mut after: Option<Json> = None;
    let outcome = loop {
        let page = match within(deadline, next_page(shared, plan, after.as_ref())).await {
            Ok(Ok(page)) => page,
            Ok(Err(s)) | Err(s) => break s,
        };
        let message = match encode(&page.answer, &response) {
            Ok(m) => frame::frame(&m),
            Err(s) => break s,
        };
        if send.is_none() {
            send = headers_sent(respond);
        }
        let Some(out) = send.as_mut() else {
            return;
        };
        match within(deadline, send_all(out, message)).await {
            Ok(Ok(())) => {}
            Ok(Err(())) => {
                out.send_reset(Reason::INTERNAL_ERROR);
                return;
            }
            Err(s) => break s,
        }
        match page.next {
            Some(next) => after = Some(next),
            None => break Status::new(status::OK, ""),
        }
    };
    match send.as_mut() {
        Some(out) => end(out, &outcome),
        None => trailers_only(respond, &outcome),
    }
}
