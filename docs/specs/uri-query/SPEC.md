# `q` URL parameter

## Serves

Parity plan R10: the stock search-adjacent REST parameters. `q` and its companions on `_search` and `_count`, as stock OpenSearch's `RestActions.urlParamsToQueryBuilder` reads them.

## Change

`oss-reader` gains `dsl::uri_query::UriQuery`, which turns `q`, `df`, `default_operator`, `analyzer`, `analyze_wildcard` and `lenient` into the one `query_string` query the same body would give. `oss-node` reads them on `GET|POST [/{index}]/_search` and `GET|POST /{index}/_count` (`search_uri_query.rs`). On `_search` a `q` replaces a query in the body, as `RestSearchAction.parseSearchSource` does; on `_count` a query in the body wins over `q`, as `RestCountAction` reads the URL only when there is no body query. The built query is checked by the `query_string` parser, so what that query refuses (`analyze_wildcard=true`, an analyzer other than `standard`, an operator other than `and` or `or`) is refused here with its message. A flag that is not `true` or `false` is a 400.

Not here: `_msearch`, `_search/template` and `_validate/query` (their routes keep refusing the parameter), and the other URL parameters of `_search` (`sort`, `_source`, `track_total_hits` and the rest).

## Acceptance criteria

- AC-1: `/{index}/_search?q=title:brown` answers the total the body `{"query":{"query_string":{"query":"title:brown"}}}` answers, on a total that is neither zero nor every document.
- AC-2: on `_search`, a `q` replaces a `query` in the body.
- AC-3: `df` and `default_operator` shape the query as the same keys in a `query_string` body do; `lenient` is accepted.
- AC-4: `/{index}/_count?q=...` counts that query; a `_count` body with a query wins over `q`.
- AC-5: the same over an index of two shards, for `_search` and `_count`.
- AC-6: an unparsable `q`, a flag that is not `true` or `false`, and an operator the query refuses answer 400.
- AC-7: the builder escapes quotes, backslashes and control characters in the text, and refuses a bad flag, operator or analyzer.

## Test plan

`crates/node/tests/it/uri_query.rs` over the compat blobs through HTTP (AC-1 to AC-6), `crates/reader/src/dsl/uri_query_tests.rs` (AC-7).

## S3 request budget

The search's; the parameter adds none. LIST 0.

## Switch

None: a parameter that was refused is now served; nothing that worked changes.

## Risks

- A `_count` with both `q` and a body `{}` counts every document here (the body has no query, so `q` applies); stock ignores `q` whenever there is a body.
- `analyze_wildcard=true` is refused because the `query_string` query refuses it; stock accepts it.
