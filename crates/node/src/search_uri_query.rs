//! The `q` URL parameter of `_search` and `_count`.
//!
//! Ported from `RestActions.urlParamsToQueryBuilder` and `RestSearchAction.parseSearchSource`
//! (server/src/main/java/org/opensearch/rest/action/RestActions.java) at OpenSearch 3.10.0: the
//! text and its companions become one `query_string` query.

use oss_reader::dsl::SearchSource;
use oss_reader::dsl::uri_query::UriQuery;
use oss_reader::dsl::url_params::UrlParams;
use oss_rest::Request;

use super::{Answer, bad_request, read_source};

/// Sets the source's query from `q` when the request has one. `keep_body_query` leaves a query the
/// body already gave (a `_count` body wins over `q`); a `_search` `q` replaces it.
pub(super) fn apply(
    req: &Request<'_>,
    source: &mut SearchSource,
    keep_body_query: bool,
) -> Result<(), Answer> {
    let Some(q) = req.param("q") else {
        return Ok(());
    };
    if keep_body_query && source.query.is_some() {
        return Ok(());
    }
    let df = req.param("df");
    let operator = req.param("default_operator");
    let analyzer = req.param("analyzer");
    let wildcard = req.param("analyze_wildcard");
    let lenient = req.param("lenient");
    let query = UriQuery {
        q: &q,
        df: df.as_deref(),
        default_operator: operator.as_deref(),
        analyzer: analyzer.as_deref(),
        analyze_wildcard: wildcard.as_deref(),
        lenient: lenient.as_deref(),
    }
    .to_query()
    .map_err(bad_request)?;
    source.query = Some(query);
    Ok(())
}

/// The query parameters `_search` takes.
pub(super) const SEARCH_PARAMS: &[&str] = &[
    "size",
    "from",
    "search_pipeline",
    "q",
    "df",
    "default_operator",
    "analyzer",
    "analyze_wildcard",
    "lenient",
    "routing",
    "preference",
    "ignore_unavailable",
    "track_total_hits",
    "terminate_after",
    "explain",
    "version",
    "seq_no_primary_term",
    "track_scores",
    "_source",
    "_source_includes",
    "_source_excludes",
    "sort",
    "search_type",
    "rest_total_hits_as_int",
    "typed_keys",
    // Accepted and without effect: one serving path, no shard copies to tune.
    "timeout",
    "request_cache",
    "batched_reduce_size",
    "pre_filter_shard_size",
    "allow_partial_search_results",
    "ccs_minimize_roundtrips",
    "max_concurrent_shard_requests",
    "allow_no_indices",
    "ignore_throttled",
    "expand_wildcards",
];

/// The body and URL parameters of a `_search`, a `q` replacing the body's query.
pub(super) fn read_search_source(req: &mut Request<'_>) -> Result<SearchSource, Answer> {
    let mut source = read_source(req)?;
    apply(req, &mut source, false)?;
    let track_total_hits = req.param("track_total_hits");
    let terminate_after = req.param("terminate_after");
    let explain = req.param("explain");
    let version = req.param("version");
    let seq_no_primary_term = req.param("seq_no_primary_term");
    let track_scores = req.param("track_scores");
    let source_param = req.param("_source");
    let includes = req.param("_source_includes");
    let excludes = req.param("_source_excludes");
    let sort = req.param("sort");
    let search_type = req.param("search_type");
    let as_int = req.param("rest_total_hits_as_int");
    let typed = req.param("typed_keys");
    let params = UrlParams {
        track_total_hits: track_total_hits.as_deref(),
        terminate_after: terminate_after.as_deref(),
        explain: explain.as_deref(),
        version: version.as_deref(),
        seq_no_primary_term: seq_no_primary_term.as_deref(),
        track_scores: track_scores.as_deref(),
        source: source_param.as_deref(),
        source_includes: includes.as_deref(),
        source_excludes: excludes.as_deref(),
        sort: sort.as_deref(),
        search_type: search_type.as_deref(),
        rest_total_hits_as_int: as_int.as_deref(),
        typed_keys: typed.as_deref(),
    };
    source.apply_url(&params).map_err(bad_request)?;
    stamp(&mut source);
    Ok(source)
}

/// Gives `source` the instant `now` means in its date math, unless it already has one (a
/// forwarded search carries its coordinator's). Taken once, where the request arrives, so every
/// shard and every item of the search reads the same instant.
pub(super) fn stamp(source: &mut SearchSource) {
    if source.now_millis.is_none() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        source.now_millis = Some(now);
    }
}
