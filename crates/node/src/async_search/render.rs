//! The answers of the asynchronous search routes, in the shapes the stock
//! plugin sends (the recording in fixtures/async-search).

use std::fmt::Write as _;

use oss_doc::mapping::json::write_str;
use oss_rest::{Exception, RestError};

use crate::jobs::{JobError, State, Stats, View};

/// What the reader of a job is told about its state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Shown {
    /// The submit that answered a finished job that is kept.
    Persisting,
    /// A read of a finished job that is kept.
    Resident,
    /// The submit that answered a finished job that is not kept.
    Closed,
}

fn state_name(view: &View, finished: Shown) -> &'static str {
    match (view.state, finished) {
        (State::Running, _) => "RUNNING",
        (_, Shown::Persisting) => "PERSISTING",
        (_, Shown::Resident) => "STORE_RESIDENT",
        (_, Shown::Closed) => "CLOSED",
    }
}

/// The error object of a job that ended in an error: a failure the search
/// reported is already one; the others are named by their kind.
fn error_object(kind: &str, text: &str) -> String {
    if kind == "search_failed" {
        return text.to_owned(); // ALLOC: one error object, bounded by the result cap.
    }
    let mut out = String::from("{\"type\":");
    write_str(&mut out, kind);
    out.push_str(",\"reason\":");
    write_str(&mut out, text);
    out.push('}');
    out
}

/// A job as the plugin answers it.
pub(super) fn envelope(view: &View, finished: Shown) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("{\"id\":");
    write_str(&mut out, &view.id);
    let _ = write!(
        out,
        ",\"state\":\"{}\",\"start_time_in_millis\":{},\"expiration_time_in_millis\":{}",
        state_name(view, finished),
        view.start,
        view.expiration
    );
    match (&view.body, &view.error) {
        (Some(body), _) => {
            out.push_str(",\"response\":");
            out.push_str(&String::from_utf8_lossy(body));
        }
        (None, Some((kind, text))) => {
            out.push_str(",\"error\":");
            out.push_str(&error_object(kind, text));
        }
        (None, None) if !view.note.is_empty() => {
            out.push_str(",\"response\":");
            out.push_str(&view.note);
        }
        (None, None) => {}
    }
    out.push('}');
    out.into_bytes()
}

/// The answer to a delete.
pub(super) fn acknowledged() -> Vec<u8> {
    b"{\"acknowledged\":true}".to_vec()
}

/// The answer to a stats call: this node's counts in the plugin's names, and
/// what only this port has apart.
pub(super) fn stats(cluster: &str, node: &str, s: &Stats) -> Vec<u8> {
    let mut out =
        String::from("{\"_nodes\":{\"total\":1,\"successful\":1,\"failed\":0},\"cluster_name\":");
    write_str(&mut out, cluster);
    out.push_str(",\"nodes\":{");
    write_str(&mut out, node);
    let _ = write!(
        out,
        ":{{\"asynchronous_search_stats\":{{\"submitted\":{},\"initialized\":{},\"running_current\":{},\"persisted\":0,\"search_failed\":{},\"search_completed\":{},\"rejected\":{},\"persist_failed\":0,\"cancelled\":{}}},\"serverless\":{{\"expired\":{},\"evicted\":{},\"held_bytes\":{}}}}}}}}}",
        s.submitted,
        s.submitted,
        s.running_current,
        s.search_failed,
        s.search_completed,
        s.rejected,
        s.cancelled,
        s.expired,
        s.evicted,
        s.held_bytes
    );
    out.into_bytes()
}

/// The refusal for a job that is not there, expired, someone else's, or on
/// a node this one cannot reach.
pub(super) fn not_found(id: &str) -> RestError {
    RestError::exception(
        404,
        // ALLOC: error path; one message.
        Exception::new(
            "resource_not_found_exception",
            format!("Either the resource [{id}] does not exist or you do not have access"),
        ),
    )
}

/// An id of a node no live lease names: 404, naming the node.
pub(super) fn unreachable(node: &str, id: &str) -> RestError {
    RestError::exception(
        404,
        // ALLOC: error path; one message.
        Exception::new(
            "resource_not_found_exception",
            format!(
                "Either the resource [{id}] does not exist or you do not have access: no live node [{node}] holds it"
            ),
        ),
    )
}

/// A relayed call that failed on the way: 503.
pub(super) fn relay_failed(node: &str) -> RestError {
    RestError::exception(
        503,
        // ALLOC: error path; one message.
        Exception::new(
            "es_rejected_execution_exception",
            format!("the call to node [{node}] failed"),
        ),
    )
}

/// A registry refusal as the client sees it.
pub(super) fn refusal(e: JobError, id: &str) -> RestError {
    match e {
        JobError::NotFound => not_found(id),
        JobError::BadRequest(reason) => RestError::illegal_argument(reason),
        JobError::Rejected(reason) => RestError::exception(
            429,
            // ALLOC: error path; one message.
            Exception::new("es_rejected_execution_exception", reason),
        ),
    }
}
