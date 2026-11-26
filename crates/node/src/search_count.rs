//! `GET|POST /{index}/_count`: how many documents match a query.
//!
//! Ported from `RestCountAction`
//! (server/src/main/java/org/opensearch/rest/action/search/RestCountAction.java)
//! at OpenSearch 3.10.0 for a body of `query` alone. The count is the exact
//! total of the search of that query with no hits, answered with the
//! search's own `_shards` object, so a sharded index counts through the
//! coordinator's merge as a search does.

use std::io::Write;
#[cfg(feature = "coordinator")]
use std::time::Instant;

use lucene_analysis::Analyzer;
use oss_blob::BlobStore;
use oss_reader::dsl::SearchSource;
use oss_reader::failure::{SearchError, ShardFailure};
use oss_reader::search::{hits_bytes, search_with};
use oss_rest::{JSON, Length, Method, Request, Response, RestError, RouteError, Router};
use std::sync::Arc;

use super::{Answer, SearchService, read_source, send, uri_query};

/// The URL parameters `_count` takes.
const COUNT_PARAMS: &[&str] = &[
    "min_score",
    "q",
    "df",
    "default_operator",
    "analyzer",
    "analyze_wildcard",
    "lenient",
];

/// The routes of `_count` on one index, by GET and POST.
pub(super) fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let get = Arc::clone(&service);
    router
        .route(
            Method::Get,
            "/{index}/_count",
            COUNT_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| get.handle_count(req, resp),
        )?
        .route(
            Method::Post,
            "/{index}/_count",
            COUNT_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle_count(req, resp),
        )
}

const NOT_SUPPORTED: &str =
    "a _count body takes only [query]; other keys are not supported by this node yet";

/// The `min_score` URL parameter: a float, where `-1` and no parameter mean no minimum, as
/// `RestCountAction` reads it.
fn min_score_param(req: &Request<'_>) -> Result<Option<f32>, RestError> {
    let Some(raw) = req.param("min_score") else {
        return Ok(None);
    };
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(if (v - -1.0).abs() < f32::EPSILON {
            None
        } else {
            Some(v)
        }),
        _ => Err(RestError::illegal_argument(format!(
            "Failed to parse float parameter [min_score] with value [{raw}]"
        ))),
    }
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Handles one `_count`.
    pub fn handle_count(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        #[cfg(feature = "coordinator")]
        let started = Instant::now();
        let mut source = match read_source(req) {
            Ok(source) => source,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let Some(index) = req.path_param("index") else {
            return Err(RestError::illegal_argument(
                "_count without an index is not supported by this node yet",
            ));
        };
        match uri_query::apply(req, &mut source, true) {
            Ok(()) => {}
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        }
        let query_only = SearchSource {
            query: source.query.clone(),
            ..SearchSource::default()
        };
        if source != query_only {
            return Err(RestError::illegal_argument(NOT_SUPPORTED));
        }
        source.min_score = min_score_param(req)?;
        source.size = 0;
        source.track_total_hits = Some(u64::from(i32::MAX.cast_unsigned()));
        #[cfg(feature = "coordinator")]
        match self.shard_count(&index) {
            Ok(shards) if shards > 1 => {
                return self.handle_sharded_with(
                    resp,
                    &index,
                    shards,
                    &source,
                    started,
                    &|bytes| count_of_search(&bytes),
                );
            }
            Ok(_) => {}
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        }
        let held = match self.shard(&index) {
            Ok(held) => held,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let (held, view) = match self.fresh_held(held, true) {
            Ok(opened) => opened,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let shard = view.as_deref().unwrap_or(&held.shard);
        req.body().reserve(hits_bytes(&source, shard))?;
        let analyzer = Analyzer::standard(None);
        let hits = match search_with(shard, &index, &source, &analyzer, held.pinned.as_deref()) {
            Ok(hits) => hits,
            Err(e) => {
                let shard_failure = ShardFailure {
                    // ALLOC: error path; the index name.
                    index: index.clone().into_owned(),
                    shard: 0,
                    // ALLOC: error path; the node id.
                    node: self.node_id.clone(),
                };
                return send(
                    resp,
                    &SearchError::on_shard(e.status, e.failure, shard_failure),
                );
            }
        };
        let mut body = resp.start(200, Some(JSON), Length::Chunked, &[])?;
        write!(
            body,
            "{{\"count\":{},\"_shards\":{{\"total\":1,\"successful\":1,\"skipped\":0,\"failed\":0}}}}",
            hits.total
        )?;
        body.flush()?;
        body.finish()
    }
}

/// The `_count` answer for a search response: its exact total and its
/// `_shards` object, copied as the search wrote it.
#[cfg(feature = "coordinator")]
fn count_of_search(response: &[u8]) -> Vec<u8> {
    const TOTAL: &[u8] = b"\"hits\":{\"total\":{\"value\":";
    const SHARDS: &[u8] = b"\"_shards\":";
    let total = find(response, TOTAL).map(|at| {
        response
            .get(at.saturating_add(TOTAL.len())..)
            .unwrap_or_default()
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .copied()
            .collect::<Vec<u8>>()
    });
    let shards = find(response, SHARDS).and_then(|at| {
        let from = at.saturating_add(SHARDS.len());
        object_len(response.get(from..)?).and_then(|n| response.get(from..from.saturating_add(n)))
    });
    // ALLOC: one answer, a count and a `_shards` object.
    let mut out = Vec::new();
    out.extend_from_slice(b"{\"count\":");
    out.extend_from_slice(total.as_deref().unwrap_or(b"0"));
    out.extend_from_slice(b",\"_shards\":");
    out.extend_from_slice(shards.unwrap_or(b"{}"));
    out.push(b'}');
    out
}

#[cfg(feature = "coordinator")]
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The length of the JSON object at the front of `bytes`, strings and
/// escapes honoured.
#[cfg(feature = "coordinator")]
fn object_len(bytes: &[u8]) -> Option<usize> {
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (at, b) in bytes.iter().enumerate() {
        if in_string {
            match (escaped, *b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match *b {
            b'"' => in_string = true,
            b'{' => depth = depth.saturating_add(1),
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(at.saturating_add(1));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(all(test, feature = "coordinator"))]
mod tests {
    use super::object_len;

    #[test]
    fn an_object_ends_at_its_matching_brace() {
        assert_eq!(object_len(b"{}rest"), Some(2));
        assert_eq!(object_len(br#"{"a":{"b":1},"c":2}tail"#), Some(19));
        assert_eq!(object_len(b"{"), None);
        assert_eq!(object_len(b""), None);
    }

    #[test]
    fn braces_inside_strings_do_not_count() {
        let object = br#"{"a":"}{","b":1}"#;
        let mut bytes = object.to_vec();
        bytes.extend_from_slice(b"}tail");
        assert_eq!(object_len(&bytes), Some(object.len()));
    }

    #[test]
    fn an_escaped_quote_does_not_end_a_string() {
        let object = br#"{"a":"x\"}y","b":"\\"}"#;
        let mut bytes = object.to_vec();
        bytes.extend_from_slice(b"}tail");
        assert_eq!(object_len(&bytes), Some(object.len()));
    }

    #[test]
    fn an_object_with_no_end_has_no_length() {
        assert_eq!(object_len(br#"{"a":"}"#), None);
        assert_eq!(object_len(br#"{"a":{"b":1}"#), None);
    }
}
