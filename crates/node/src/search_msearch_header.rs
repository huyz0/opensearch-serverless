//! The header line of an `_msearch` item: the index it names and the keys it sets.
//!
//! Ported from `RestMultiSearchAction.parseMultiLineRequest` (server/src/main/java/org/opensearch/rest/action/search/RestMultiSearchAction.java) at OpenSearch 3.10.0.

use oss_reader::dsl::Recorder;
use oss_reader::dsl::tokens::Kind;
use oss_rest::RestError;

use super::{not_supported, parse_flag, rest_item};

/// Header keys accepted without effect, whose value is any one scalar.
const NO_EFFECT_KEYS: &[&str] = &[
    "request_cache",
    "ccs_minimize_roundtrips",
    "allow_no_indices",
    "max_concurrent_shard_requests",
    "pre_filter_shard_size",
    "ignore_throttled",
];

/// The index a header names, or why this node cannot search it.
pub(super) fn header_index(
    rec: &Recorder,
    parts: bool,
    default: Option<&str>,
) -> Result<Header, Vec<u8>> {
    let mut named: Option<String> = None;
    let mut routing: Option<String> = None;
    let mut ignore_unavailable = false;
    if parts {
        let mut p = rec.parser();
        if p.next_token() != Some(Kind::StartObject) {
            return Err(rest_item(&RestError::illegal_argument(
                "a _msearch header must be a JSON object",
            )));
        }
        while p.next_token() == Some(Kind::FieldName) {
            let key = p.text().to_owned();
            if !matches!(
                key.as_str(),
                "index"
                    | "routing"
                    | "preference"
                    | "ignore_unavailable"
                    | "search_type"
                    | "expand_wildcards"
            ) && !NO_EFFECT_KEYS.contains(&key.as_str())
            {
                return Err(not_supported(&format!("the _msearch header key [{key}]")));
            }
            let token = p.next_token();
            if key == "ignore_unavailable" && token == Some(Kind::Bool) {
                ignore_unavailable = p.text() == "true";
                continue;
            }
            if NO_EFFECT_KEYS.contains(&key.as_str()) {
                if !matches!(token, Some(Kind::String | Kind::Bool | Kind::Number)) {
                    return Err(not_supported(&format!(
                        "an _msearch [{key}] that is not one value"
                    )));
                }
                continue;
            }
            if token != Some(Kind::String) {
                return Err(not_supported(&format!(
                    "an _msearch [{key}] that is not one string"
                )));
            }
            match key.as_str() {
                "index" => named = Some(p.text().to_owned()),
                "routing" => routing = Some(p.text().to_owned()),
                "ignore_unavailable" => {
                    ignore_unavailable = parse_flag(Some(p.text())).map_err(|e| rest_item(&e))?;
                }
                "search_type" => check_search_type(p.text()).map_err(|e| rest_item(&e))?,
                "expand_wildcards" if p.text() != "open" => {
                    return Err(not_supported(
                        "an _msearch [expand_wildcards] other than open",
                    ));
                }
                // No shard has a copy to prefer.
                _ => {}
            }
            p.skip_children();
        }
    }
    let index = named
        .or_else(|| default.map(str::to_owned))
        .ok_or_else(|| not_supported("an _msearch item with no index"))?;
    if index.is_empty() || index == "_all" || index.contains('*') {
        return Err(not_supported("a pattern or _all in _msearch"));
    }
    Ok(Header {
        index,
        routing,
        ignore_unavailable,
    })
}

/// `search_type`: only the default is served.
pub(super) fn check_search_type(text: &str) -> Result<(), RestError> {
    match text {
        "query_then_fetch" => Ok(()),
        "dfs_query_then_fetch" => Err(RestError::illegal_argument(
            "the search type [dfs_query_then_fetch] is not supported by this node yet",
        )),
        other => Err(RestError::illegal_argument(format!(
            "No search type for [{other}]"
        ))),
    }
}

/// What an item's header asks for.
pub(super) struct Header {
    pub(super) index: String,
    pub(super) routing: Option<String>,
    pub(super) ignore_unavailable: bool,
}
