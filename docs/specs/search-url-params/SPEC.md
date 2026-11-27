# Common URL parameters on `_search`

## Serves

The URL-parameter gap of [compatibility](../../compatibility.md): clients and the official language clients send `track_total_hits`, `terminate_after`, `explain`, `version`, `seq_no_primary_term`, `track_scores`, `_source`, `_source_includes`, `_source_excludes`, `sort` and `timeout` as URL parameters, and a `_search` naming any of them answers 400 (unknown parameter).

## Change

- Each of those parameters sets the same setting its body key sets (`RestSearchAction.parseSearchSource`), and the URL value wins over the body's.
- `track_total_hits` is `true`, `false` or a number; `terminate_after` a number of at least 0; `explain`, `version`, `seq_no_primary_term` and `track_scores` are `true` or `false`.
- `_source` is `true`, `false` or a comma-separated list of patterns to include; `_source_includes` and `_source_excludes` are lists and replace the includes and excludes of `_source`.
- `sort` is a comma-separated list of `field` or `field:asc` or `field:desc`; `_score` sorts by score.
- Accepted and without effect, because the node has one serving path and no shard copies to tune: `timeout`, `request_cache`, `batched_reduce_size`, `pre_filter_shard_size`, `allow_partial_search_results`, `ccs_minimize_roundtrips`, `max_concurrent_shard_requests`, and `search_type=query_then_fetch`.
- Still refused as an unknown parameter: `typed_keys`, `stored_fields`, `docvalue_fields`. `rest_total_hits_as_int` is [its own spec](../search-total-hits-as-int/SPEC.md). `search_type=dfs_query_then_fetch` answers 400 "not supported by this node yet".
- Not here: the parameters of `_msearch` headers, `_count` and point-in-time searches.

## Acceptance criteria

- AC-1: `?track_total_hits=false` answers no total, `?track_total_hits=1` caps the total at 1 with relation `gte`, and either overrides a body that says otherwise.
- AC-2: `?explain=true` gives each hit an `_explanation`, `?version=true` a `_version`, `?seq_no_primary_term=true` a `_seq_no` and `_primary_term`, and `?track_scores=true` a score on a field-sorted hit.
- AC-3: `?terminate_after=1` collects at most one match a shard, and `?terminate_after=-1` answers 400.
- AC-4: `?_source=false` drops the source, `?_source=title` and `?_source_includes=title` keep only that field, `?_source_excludes=title` drops it.
- AC-5: `?sort=n:desc` and `?sort=n` order the hits as the body's `sort` does, and the URL sort replaces the body's; `?sort=n:sideways` answers 400 "sort direction [sideways] not supported".
- AC-6: `?timeout=5s`, `?request_cache=true`, `?batched_reduce_size=512`, `?pre_filter_shard_size=1`, `?allow_partial_search_results=true`, `?ccs_minimize_roundtrips=true`, `?max_concurrent_shard_requests=5` and `?search_type=query_then_fetch` answer what the search without them answers.
- AC-7: `?search_type=dfs_query_then_fetch` answers 400, and `?typed_keys=true` is still refused.
- AC-8: a boolean parameter that is not `true` or `false`, and a `track_total_hits` that is neither a boolean nor a number, answer 400 with OpenSearch's messages.

## S3 request budget

None added: the parameters change what a search reads from the descriptor and segments it already opens, and `terminate_after` and `track_total_hits=false` only read fewer.

## Switch

None: every parameter turns a refusal into the answer its body key already gives.

## Tests

`crates/node/tests/it/search_url_params.rs` through HTTP, and the reader's `url_params` unit tests for the parsing of each value.
