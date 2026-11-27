# `_msearch`

## Serves

Parity plan R10: the stock search-adjacent REST APIs. `GET|POST /_msearch` and `/{index}/_msearch` run the searches of a newline-delimited body, a header line then a body line each, as stock OpenSearch's `RestMultiSearchAction` does for the header keys in scope.

## Change

`oss-node` (feature `reader`) gains `search_msearch`: the four routes, registered by one line in `search::routes`, and `SearchService::handle_msearch`. The body streams a line at a time through the NDJSON splitter; each header names the index (`index`, a single concrete name; the path's index is the default) and each body line is a search body, parsed as `_search` parses it. Searches run one after another as their lines complete, each answered with the search's own response bytes plus a final `"status":200`, or with its error body (which carries its `status`). One failing search never fails the others. The answer is `{"responses":[...],"took":N}` written as the searches finish.

Not here: header keys other than `index`, `routing` and `preference` ([search-routing](../search-routing/SPEC.md)) (`search_type`, `request_cache`, `allow_no_indices`, `ignore_unavailable`, `expand_wildcards`, `ccs_minimize_roundtrips`), an `index` that is an array, a pattern, `_all` or a comma list, a search with no index, and every URL parameter (`max_concurrent_searches`, `rest_total_hits_as_int` and the rest). Each refused header answers its item as a 400 `illegal_argument_exception` ("... is not supported by this node yet") and its body line is skipped.

## Acceptance criteria

- AC-1: a body of two searches answers `responses` of two items, each the response its `_search` gives (its `took` aside) with `"status":200` as its last key, then `took`.
- AC-2: the path's index is the default for a header without one; a header's `index` overrides it.
- AC-3: an item that fails (unknown index 404, a query the search refuses 400) answers its own error body with its status in its place, and the others are unaffected; the response is 200.
- AC-4: a header key other than `index`, an `index` that is an array, a pattern, `_all` or a comma list, and a header with no index and no default each answer a 400 `illegal_argument_exception` item saying it is not supported by this node yet.
- AC-5: an empty body answers 400 `action_request_validation_exception` ("no requests added"); a header without its body line, and a body not ended by a newline, answer a 400 item (a 400 response when no item has been answered).
- AC-6: over an index of two shards an item is the coordinator's `_search` response.
- AC-7: an unknown URL parameter answers the router's 400, and a run over a warm open index makes no object-store request.

## Test plan

`crates/node/tests/it/msearch.rs` over the compat blobs (`compat-term` and its two-shard variant), through HTTP; each expected item is the same search through `_search`. AC-7 reads the meter, as `it/count.rs` does.

## S3 request budget

The searches': a cold open of each named index is the descriptor, the commit and its ranged read; warm open indices cost nothing. LIST 0. Requests scale with the searches in the body, which the body size bounds.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- Searches run one at a time, so `max_concurrent_searches` has nothing to do; memory is one search's at a time, reserved from the request-body pool and released as its item is written.
- Because the answer streams, `took` is the last key rather than the first, and a fault found after the first item is answered as an item, not as a 400. Listed under compatibility.
- A malformed JSON header or body line answers its item as a 400 item and the following lines continue; OpenSearch fails the whole request.
