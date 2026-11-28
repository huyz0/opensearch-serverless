//! The runner over the node's own search: a job is one `_search` of the
//! reader, with the caller's index scope.

use std::sync::Arc;

use oss_reader::dsl::parse_chunks;
use oss_rest::{Exception, RestError};
use serde_json::Value;

use super::{Plan, Runner, Submit};
use crate::jobs::{Ctx, Failure, Work};
use crate::sql_route::{SqlBackend, SqlFailure};

/// The partial response a running search shows: no shard has answered.
const PARTIAL: &str = r#"{"took":0,"timed_out":false,"_shards":{"total":0,"successful":0,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"max_score":null,"hits":[]}}"#;

/// Plans each submit as one search of the node's own reader.
pub struct SearchRunner {
    backend: Arc<dyn SqlBackend>,
}

impl SearchRunner {
    /// A runner over `backend`, the node's search.
    #[must_use]
    pub fn new(backend: Arc<dyn SqlBackend>) -> Self {
        Self { backend }
    }
}

/// The index names of a path expression: every comma-separated name, or all
/// indices when the path has none.
fn names(index: Option<&str>) -> Vec<String> {
    match index {
        Some(expr) => expr.split(',').map(str::to_owned).collect(), // ALLOC: one name per index of the path.
        None => vec!["*".to_owned()],                               // ALLOC: one name.
    }
}

impl Runner for SearchRunner {
    fn plan(&self, submit: &Submit<'_>, _search: &[(String, String)]) -> Result<Plan, RestError> {
        // A body that is not JSON, or null, is a match-all, as the stock
        // plugin reads it.
        let body: Value = serde_json::from_slice(submit.body)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        let bytes = body.to_string().into_bytes();
        if let Err(e) = parse_chunks([bytes.as_slice()]) {
            let status = e.status();
            let failure = e.into_failure();
            return Err(RestError::exception(
                status,
                Exception::new(failure.kind, failure.reason),
            ));
        }
        let backend = Arc::clone(&self.backend);
        let names = names(submit.index);
        let work: Work = Box::new(move |ctx: &Ctx| {
            if ctx.should_stop() {
                return Err(Failure::Stopped);
            }
            match backend.search_until(&names, &body, &|| ctx.should_stop()) {
                Ok(bytes) if bytes.len() > ctx.max_bytes() => Err(Failure::TooLarge),
                Ok(bytes) => Ok(bytes),
                // The search gave up at a shard boundary because it was told to.
                Err(_) if ctx.should_stop() => Err(Failure::Stopped),
                Err(SqlFailure::Body(_, body)) => Err(Failure::Error(error_object(&body))),
                Err(SqlFailure::Rest(e)) => {
                    let mut out = Vec::new();
                    let _ = e.write_body(&mut out);
                    Err(Failure::Error(error_object(&out)))
                }
            }
        });
        Ok(Plan {
            work,
            partial: PARTIAL.to_owned(), // ALLOC: one constant, once per job.
        })
    }
}

/// The `error` object of a failed search's body, as text.
fn error_object(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").cloned())
        .map_or_else(
            || r#"{"type":"search_failed","reason":"the search failed"}"#.to_owned(), // ALLOC: one constant on the error path.
            |e| e.to_string(),
        )
}
