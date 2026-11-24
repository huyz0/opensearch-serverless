//! `GET|POST /{index}/_validate/query`: whether a query is valid for an index.
//!
//! Ported from `RestValidateQueryAction` and `TransportValidateQueryAction`
//! (server/src/main/java/org/opensearch/rest/action/admin/indices/ and
//! action/admin/indices/validate/query/) at OpenSearch 3.10.0 for a body of
//! `query` alone and no explanation. The query is parsed and built over the
//! mapping of shard 0 as a search builds it, and nothing is searched. Stock
//! routes a validation to one shard of each index, so `_shards` counts one.

use std::sync::Arc;

use lucene_analysis::Analyzer;
use oss_blob::BlobStore;
use oss_reader::dsl::{Query, SearchSource};
use oss_reader::failure::Failure;
use oss_reader::lucene_query::{QueryContext, shard_failure, to_lucene};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use super::{Answer, SearchService, read_source, send};

/// The URL parameters `_validate/query` takes: none yet.
const VALIDATE_PARAMS: &[&str] = &[];

/// What a refusal of this node says, and so what is not an invalid query.
const NOT_SUPPORTED: &str = "is not supported by this node yet";

const VALID: &[u8] = br#"{"_shards":{"total":1,"successful":1,"failed":0},"valid":true}"#;
const INVALID: &[u8] = br#"{"_shards":{"total":1,"successful":1,"failed":0},"valid":false}"#;

/// The routes of `_validate/query`, by GET and POST, with and without an
/// index; the form without one answers the refusal.
pub(super) fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Get, Method::Post] {
        for path in ["/{index}/_validate/query", "/_validate/query"] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                VALIDATE_PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle_validate(req, resp)
                },
            )?;
        }
    }
    Ok(router)
}

/// Whether `f` or one of its causes is this node's refusal.
fn refused(f: &Failure) -> bool {
    let mut at = Some(f);
    while let Some(f) = at {
        if f.reason.contains(NOT_SUPPORTED) {
            return true;
        }
        at = f.cause.as_deref();
    }
    false
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Handles one `_validate/query`.
    pub fn handle_validate(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let parsed = match read_source(req) {
            Ok(source) => Some(source),
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) if refused(&e.failure) => return send(resp, &e),
            Err(Answer::Search(_)) => None,
        };
        let Some(index) = req.path_param("index") else {
            return Err(RestError::illegal_argument(
                "_validate/query without an index is not supported by this node yet",
            ));
        };
        let held = match self.open_shard(&index, 0, false) {
            Ok(held) => held,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let valid = match parsed {
            Some(source) => {
                let query_only = SearchSource {
                    query: source.query.clone(),
                    ..SearchSource::default()
                };
                if source == query_only {
                    let shard = &held.shard;
                    let analyzer = Analyzer::standard(None);
                    let cx = QueryContext {
                        mapping: &shard.meta.mapping,
                        analyzer: &analyzer,
                        max_doc: shard.reader.max_doc(),
                        knn: None,
                    };
                    let query = source.query.unwrap_or(Query::MatchAll { boost: 1.0 });
                    match to_lucene(&query, &cx) {
                        Ok(_) => true,
                        Err(f) => {
                            let f = shard_failure(f, &index, &shard.meta.uuid);
                            if refused(&f) {
                                return resp.send(400, JSON, &refusal_body(&f));
                            }
                            false
                        }
                    }
                } else {
                    false
                }
            }
            None => false,
        };
        resp.send(200, JSON, if valid { VALID } else { INVALID })
    }
}

/// The 400 body of a refusal found while building the query.
fn refusal_body(f: &Failure) -> Vec<u8> {
    oss_reader::failure::SearchError::request(400, f.clone()).to_body()
}
