//! `GET|POST /detectors/results/_search[/{result_index}]`: a search of the
//! result index the runs write to, through the node's own search.

use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::run::RESULT_INDEX;
use super::{AdService, bad};
use crate::route_util::{read_body, refuse};

/// The most bytes of a search body.
const MAX_QUERY_BYTES: usize = 1 << 16;

/// The most bytes of an answer, held until it is written.
const MAX_ANSWER_BYTES: usize = 8 << 20;

/// The answer of a search over an index that does not exist yet: no run has
/// written a result, so there is nothing to find.
fn empty() -> Value {
    json!({
        "took": 0,
        "timed_out": false,
        "_shards": {"total": 0, "successful": 0, "skipped": 0, "failed": 0},
        "hits": {"total": {"value": 0, "relation": "eq"}, "max_score": 0.0, "hits": []}
    })
}

impl AdService {
    /// A search of the result index, or of `{result_index}`, as the body
    /// asks; an index no run has written is an empty answer.
    pub(crate) fn results(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let raw = read_body(
            req,
            MAX_QUERY_BYTES,
            "the body is over the limit for a search",
        )?;
        let body: Value = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?
        };
        let search = self.search.as_ref().ok_or_else(|| {
            refuse(
                500,
                "ad_source_unavailable",
                "this node cannot search for results",
            )
        })?;
        let index = req
            .path_param("result_index")
            .map_or_else(|| RESULT_INDEX.to_owned(), |i| i.to_string());
        // The answer is held until it is written.
        req.body().reserve(MAX_ANSWER_BYTES)?;
        let answer = match search.search(&[index], &body) {
            Ok(answer) => answer,
            Err(e) if e.status() == 404 => empty(),
            Err(e) => return Err(e),
        };
        resp.send(200, JSON, answer.to_string().as_bytes())
    }
}
