//! What a delete-by-query request says: its query and its parameters.

use oss_rest::{Request, RestError};

use crate::bulk_route::refresh;

/// The most bytes of a request body.
const MAX_BODY_BYTES: usize = 1 << 20;

/// The query parameters a delete-by-query accepts. Those with nothing to do
/// here (one node, no throttle) are accepted at their defaults only.
pub(super) const PARAMS: &[&str] = &[
    "conflicts",
    "max_docs",
    "refresh",
    "wait_for_completion",
    "scroll_size",
    "timeout",
    "slices",
    "requests_per_second",
];

/// A parsed request.
pub(super) struct Params {
    /// The `query` object, as JSON text.
    pub query: String,
    /// `max_docs`: the most documents deleted.
    pub max_docs: u64,
    /// `refresh`.
    pub refresh: bool,
    /// `conflicts=abort`, the default.
    pub abort_on_conflict: bool,
}

fn bad(reason: impl Into<String>) -> RestError {
    RestError::illegal_argument(reason)
}

fn unsupported(what: &str) -> RestError {
    // ALLOC: error path; one message.
    bad(format!("{what} is not supported by this node"))
}

impl Params {
    /// Reads the request, refusing what this node does not do.
    pub fn read(req: &mut Request<'_>) -> Result<Self, RestError> {
        let abort_on_conflict = match req.param("conflicts").as_deref() {
            None | Some("abort") => true,
            Some("proceed") => false,
            Some(other) => {
                // ALLOC: error path; one message.
                return Err(bad(format!(
                    "conflicts may only be \"proceed\" or \"abort\" but was [{other}]"
                )));
            }
        };
        if req
            .param("wait_for_completion")
            .is_some_and(|v| v == "false")
        {
            return Err(unsupported("wait_for_completion=false"));
        }
        if req.param("slices").is_some_and(|v| v != "1") {
            return Err(unsupported("slices other than 1"));
        }
        if req.param("requests_per_second").is_some_and(|v| v != "-1") {
            return Err(unsupported("throttling"));
        }
        let max_docs = match req.param("max_docs") {
            None => u64::MAX,
            Some(text) => match text.parse::<i64>() {
                Ok(n) if n >= 0 => u64::try_from(n).unwrap_or(u64::MAX),
                _ => {
                    // ALLOC: error path; one message.
                    return Err(bad(format!(
                        "[max_docs] parameter cannot be negative, found [{text}]"
                    )));
                }
            },
        };
        let refresh = refresh(req.param("refresh").as_deref())?;
        let body = read_body(req)?;
        let value = oss_doc::mapping::json::parse(&body).map_err(|e| {
            // ALLOC: error path; one message.
            bad(format!("failed to parse the request body: {}", e.reason))
        })?;
        let Some(query) = value.get("query") else {
            return Err(bad("query is missing"));
        };
        Ok(Self {
            query: query.to_json(),
            max_docs,
            refresh,
            abort_on_conflict,
        })
    }
}

/// The body, read as it streams in, its growth reserved first.
fn read_body(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
    let body = req.body();
    // ALLOC: the body so far, at most MAX_BODY_BYTES, reserved as it grows.
    let mut out: Vec<u8> = Vec::new();
    loop {
        let more = out
            .len()
            .saturating_add(body.max_chunk())
            .saturating_sub(body.reserved());
        body.reserve(more)?;
        let Some(next) = body.next_chunk()? else {
            break;
        };
        if out.len().saturating_add(next.len()) > MAX_BODY_BYTES {
            return Err(bad("the body of a delete by query is larger than 1 MiB"));
        }
        out.extend_from_slice(next);
    }
    if out.is_empty() {
        return Err(bad("request body is required"));
    }
    Ok(out)
}
