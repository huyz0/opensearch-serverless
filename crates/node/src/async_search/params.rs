//! The query parameters of the asynchronous search routes, read as the stock
//! plugin reads them (the recording in fixtures/async-search).

use oss_rest::{Exception, RestError};

use crate::time_value::time_millis;

/// The wait a submit makes when it names none.
const DEFAULT_WAIT_MS: u64 = 1_000;
/// The longest wait this node allows.
pub(super) const MAX_WAIT_MS: u64 = 10_000;
/// The shortest `keep_alive`.
const MIN_KEEP_ALIVE_MS: u64 = 60_000;
/// Search parameters a submit passes on to the search.
const SEARCH_PARAMS: &[&str] = &[
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

/// A submit's parameters.
pub(super) struct Submit {
    pub wait_ms: u64,
    pub keep: bool,
    pub keep_alive_ms: Option<u64>,
    /// The ones that belong to the search.
    pub search: Vec<(String, String)>,
}

/// A read's parameters.
pub(super) struct Read {
    pub wait_ms: u64,
    pub keep_alive_ms: Option<u64>,
}

fn unknown(path: &str, name: &str) -> RestError {
    // ALLOC: error path; one message.
    RestError::illegal_argument(format!(
        "request [{path}] contains unrecognized parameter: [{name}]"
    ))
}

fn flag(value: &str) -> Result<bool, RestError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        // ALLOC: error path; one message.
        _ => Err(RestError::illegal_argument(format!(
            "Failed to parse value [{value}] as only [true] or [false] are allowed."
        ))),
    }
}

fn wait(value: &str) -> Result<u64, RestError> {
    let ms = time_millis("wait_for_completion_timeout", value)?;
    if ms > MAX_WAIT_MS {
        // ALLOC: error path; one message.
        return Err(RestError::illegal_argument(format!(
            "wait_for_completion_timeout [{value}] is longer than this node allows ({MAX_WAIT_MS}ms)"
        )));
    }
    Ok(ms)
}

fn keep_alive(value: &str, max_ms: u64) -> Result<u64, RestError> {
    let ms = time_millis("keep_alive", value)?;
    if ms < MIN_KEEP_ALIVE_MS {
        return Err(RestError::exception(
            400,
            // ALLOC: error path; one message.
            Exception::new(
                "action_request_validation_exception",
                format!(
                    "Validation Failed: 1: [keep_alive] must be greater than 1 minute, got: {value};"
                ),
            ),
        ));
    }
    if ms > max_ms {
        // ALLOC: error path; one message.
        return Err(RestError::illegal_argument(format!(
            "Keep alive for asynchronous search ({ms}) is too large. It must be less than ({max_ms}ms).This limit can be set by changing the [serverless.async_search.max_keep_alive] setting."
        )));
    }
    Ok(ms)
}

/// Reads a submit's parameters.
///
/// # Errors
///
/// 400 for a malformed value, a `keep_alive` outside its bounds or an
/// unknown parameter.
pub(super) fn submit(
    query: &[(String, String)],
    path: &str,
    max_keep_alive_ms: u64,
) -> Result<Submit, RestError> {
    let mut out = Submit {
        wait_ms: DEFAULT_WAIT_MS,
        keep: false,
        keep_alive_ms: None,
        search: Vec::new(),
    };
    for (name, value) in query {
        match name.as_str() {
            "wait_for_completion_timeout" => out.wait_ms = wait(value)?,
            "keep_on_completion" => out.keep = flag(value)?,
            "keep_alive" => out.keep_alive_ms = Some(keep_alive(value, max_keep_alive_ms)?),
            n if SEARCH_PARAMS.contains(&n) => out.search.push((name.clone(), value.clone())), // ALLOC: at most the parameters of a request.
            _ => return Err(unknown(path, name)),
        }
    }
    Ok(out)
}

/// Reads a get's parameters.
///
/// # Errors
///
/// As [`submit`].
pub(super) fn read(
    query: &[(String, String)],
    path: &str,
    max_keep_alive_ms: u64,
) -> Result<Read, RestError> {
    let mut out = Read {
        wait_ms: 0,
        keep_alive_ms: None,
    };
    for (name, value) in query {
        match name.as_str() {
            "wait_for_completion_timeout" => out.wait_ms = wait(value)?,
            "keep_alive" => out.keep_alive_ms = Some(keep_alive(value, max_keep_alive_ms)?),
            _ => return Err(unknown(path, name)),
        }
    }
    Ok(out)
}
