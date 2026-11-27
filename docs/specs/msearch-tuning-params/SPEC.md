# Tuning parameters and `rest_total_hits_as_int` on `_msearch`

## Serves

The `_msearch` gap of [compatibility](../../compatibility.md): the official clients send `max_concurrent_searches`, `search_type` and the like on the URL and `search_type`, `request_cache` and the like in each header, and every one answered 400 or "not supported by this node yet".

## Change

- URL parameters `max_concurrent_searches`, `max_concurrent_shard_requests`, `pre_filter_shard_size`, `ccs_minimize_roundtrips` and `search_type=query_then_fetch` are accepted and change nothing: a node runs the items one at a time on one serving path.
- Header keys `request_cache`, `ccs_minimize_roundtrips`, `allow_no_indices`, `max_concurrent_shard_requests`, `pre_filter_shard_size`, `ignore_throttled` (a string, a boolean or a number) and `search_type` of `query_then_fetch` and `expand_wildcards` of `open` are accepted and change nothing.
- `search_type` of `dfs_query_then_fetch` answers "not supported by this node yet", any other value "No search type for [x]", and `expand_wildcards` of any other value "not supported" (patterns are refused anyway): on the URL as a 400, in a header as that item's 400.
- `?rest_total_hits_as_int=true` writes every item's total as a number, with the rules of [search-total-hits-as-int](../search-total-hits-as-int/SPEC.md); an item whose `track_total_hits` is a count or `false` answers its own 400.
- Not here: `typed_keys`, and the header keys not listed.

## Acceptance criteria

- AC-1: each tuning header key above answers the item as the header `{}` does.
- AC-2: `search_type` of `dfs_query_then_fetch` or an unknown type, and `expand_wildcards` of `closed`, answer the item's 400.
- AC-3: each tuning URL parameter above answers as the request without it does; `search_type=dfs_query_then_fetch` and `typed_keys` answer 400.
- AC-4: `?rest_total_hits_as_int=true` writes the item's total as a number.

## S3 request budget

None added: nothing here reads a blob.

## Switch

None: refusals become answers.

## Tests

`crates/node/tests/it/search_url_params.rs`, through HTTP.
