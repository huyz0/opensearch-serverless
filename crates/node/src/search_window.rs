//! The node's search as the source of an anomaly detector's windows: one
//! search over the detector's indexes, answered as JSON.

use oss_blob::BlobStore;
use oss_reader::dsl::{MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_rest::RestError;
use serde_json::Value;

use super::{Answer, SearchService, bad_request};
use crate::ad_route::WindowSearch;
use crate::route_util::refuse;

/// The most bytes of a failed search's body a refusal repeats.
const MAX_REASON_BYTES: usize = 512;

fn refused(answer: Answer) -> RestError {
    match answer {
        Answer::Rest(e) => e,
        Answer::Search(e) => {
            let body = e.to_body();
            let text = String::from_utf8_lossy(body.get(..MAX_REASON_BYTES).unwrap_or(&body));
            // ALLOC: error path; one message.
            refuse(
                e.status,
                "ad_search_failure",
                format!("the detector's search failed: {text}"),
            )
        }
    }
}

impl<S: BlobStore + 'static> WindowSearch for SearchService<S> {
    fn search(&self, indices: &[String], body: &Value) -> Result<Value, RestError> {
        let targets = self.expand_names(indices.iter().map(String::as_str))?;
        // ALLOC: the request's text, once.
        let text = serde_json::to_vec(body)
            .map_err(|e| RestError::illegal_argument(format!("the detector's search: {e}")))?;
        let mut rec = Recorder::new(MAX_RECORDED_BYTES);
        let mut tokens = source::tokenizer();
        tokens
            .feed(&text, &mut rec)
            .map_err(|e| refused(bad_request(e)))?;
        tokens
            .finish(&mut rec)
            .map_err(|e| refused(bad_request(e)))?;
        let source = SearchSource::parse(&rec).map_err(|e| refused(bad_request(e)))?;
        let bytes = match targets.as_slice() {
            [one] => self.search_bytes(one, &source),
            many => self.search_bytes_many(many, &source, None, None),
        }
        .map_err(refused)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| RestError::illegal_argument(format!("the search answer: {e}")))
    }
}
