# Wildcard options on `_search` and `_count`, and `_search` URL parameters over a view

## Serves

Clients send `allow_no_indices`, `ignore_throttled` and `expand_wildcards` on `_search` and `_count`, and both answered 400 (unknown parameter). The URL parameters of [search-url-params](../search-url-params/SPEC.md) and [search-total-hits-as-int](../search-total-hits-as-int/SPEC.md) are also asked of a search of a point in time.

## Change

- `allow_no_indices` and `ignore_throttled` are accepted on `_search` and `_count` and change nothing: a pattern is refused, so there is nothing for them to apply to, and a node has no throttled indices.
- `expand_wildcards=open` is accepted; any other value answers 400 "an [expand_wildcards] other than open is not supported by this node yet".
- A `_search` over a point in time reads the same URL parameters as any `_search` (`track_total_hits`, `rest_total_hits_as_int`, `_source`, `size`, `timeout`, `search_type=query_then_fetch` and the rest), because the view is searched from the same parsed source; `routing` and `preference` still answer 400 with a view.
- Not here: `terminate_after` on `_count`, which would need the scoring query a count over `match_all` lacks.

## Acceptance criteria

- AC-1: `allow_no_indices`, `ignore_throttled` and `expand_wildcards=open` on `_search` answer what the search without them answers.
- AC-2: `expand_wildcards=closed` and `=all` answer 400 on `_search` and on `_count`; on `_count` the three accepted options answer 200.
- AC-3: a point-in-time search with `rest_total_hits_as_int`, `track_total_hits=false`, `_source=false`, `size`, `timeout` and `search_type=query_then_fetch` answers as the same parameters answer a live search.

## S3 request budget

None added: nothing here reads a blob.

## Switch

None: refusals become answers.

## Tests

`crates/node/tests/it/search_url_params.rs` and `crates/node/tests/it/frozen_search.rs`, through HTTP.
