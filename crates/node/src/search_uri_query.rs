//! The `q` URL parameter of `_search` and `_count`.
//!
//! Ported from `RestActions.urlParamsToQueryBuilder` and `RestSearchAction.parseSearchSource`
//! (server/src/main/java/org/opensearch/rest/action/RestActions.java) at OpenSearch 3.10.0: the
//! text and its companions become one `query_string` query.

use oss_reader::dsl::SearchSource;
use oss_reader::dsl::uri_query::UriQuery;
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
];

/// The body and URL parameters of a `_search`, a `q` replacing the body's query.
pub(super) fn read_search_source(req: &mut Request<'_>) -> Result<SearchSource, Answer> {
    let mut source = read_source(req)?;
    apply(req, &mut source, false)?;
    Ok(source)
}
