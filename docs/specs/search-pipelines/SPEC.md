# Search pipelines

## Serves

The search pipelines piece of the snapshots, templates, ingest and auth milestone (R14.6): `PUT|GET|DELETE /_search/pipeline/{id}`, `GET /_search/pipeline`, and the `search_pipeline` request parameter on `_search`. The ingest counterpart is [pipeline-routes](../pipeline-routes/SPEC.md) and [ADR 0044](../../adr/0044-ingest-pipelines-in-the-write-path.md); the allowlist approach is the same.

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `rest/SearchPipelineHandler.java` (routes and refusals), `rest/SearchPlanner.java` (the `search_pipeline` parameter, `pipeline_missing`) and the common module's `OversampleRequestProcessor`, `TruncateHitsResponseProcessor` and `FilterQueryRequestProcessor`. The Java compiles a pipeline with core's engine; this build has its own small one (`oss_reader::search_pipeline`), so it runs exactly the processors below, and refuses the rest when the pipeline is stored.

- **Storage.** Pipelines are a document set ([doc-sets](../doc-sets/SPEC.md), kind `SearchPipeline`, `search_pipelines/`). `PUT` replaces, `DELETE` removes (404 `pipeline_missing` when absent), `GET` selects all (no id, `*`, `_all`) or one id; several ids, other wildcards and underscore ids are 501, as for ingest pipelines. Only `PUT` stores (as in the Java), not `POST`.
- **Definition.** An object of `description`, `version`, `request_processors` and `response_processors` (arrays of one-key objects). Supported: request `oversample` (`sample_factor` >= 1.0, optional `context_prefix`) and `filter_query` (`query`); response `truncate_hits` (optional `target_size` >= 0, optional `context_prefix`). A processor's `tag` and `description` are accepted; any other option is refused 400 `invalid_pipeline`. A processor OpenSearch ships and this node does not build (`script`, `rename_field`, `collapse`, `sort`, `split`, `acl_routing_search`, `hierarchical_routing_search`, `temporal_routing_search`), a non-empty `phase_results_processors`, and `ignore_failure: true` are refused 501 `not_implemented`, naming what was refused and listing what is supported. A name OpenSearch has no processor of, and a malformed body, are 400 `invalid_pipeline`.
- **Running.** The response side is folded into the request: `truncate_hits` depends only on its own option and what `oversample` recorded, so the cut is fixed before the search runs (`SearchSource::response_size_cap`) and the hits written are cut to it; the result is the stock one, and a `truncate_hits` that can never succeed fails the search before it spends any request. `?search_pipeline=<id>` on `_search` (with or without an index, single shard, sharded and point-in-time paths) runs the stored pipeline: request processors in order over the parsed search before it runs, then response processors over the hits before they are written. `_none` runs nothing. `oversample` sets `size` to ceil(size x factor) and remembers the original size under `<context_prefix>original_size`; `truncate_hits` cuts the hits to `target_size`, or to that remembered size, and fails 500 `search_pipeline_processing_exception` when neither exists; `filter_query` makes the query a `bool` with the old query as `must` (none when the search had none) and the configured query as `filter`. The result window is checked after the request processors, as in OpenSearch.
- **Missing pipeline.** A named pipeline that is not stored answers 400 `pipeline_missing`, `no such search pipeline: <id>`, as the Java shell does.
- **Not changed.** `index.search.default_pipeline` stays refused 501 on a settings update, as in the shell (its words already say to use `?search_pipeline=`); a `search_pipeline` object in the body, `_msearch`, `_count` and `_search/template` keep refusing it. These are follow-ups: a default needs a settings read on every search and a rule for several indices with different defaults, which the shell has not decided either.

## Acceptance criteria

- AC-1: A definition of supported processors is accepted; an unknown name, a processor with two keys, a bad option value, a bad `sample_factor` or `target_size`, and a body that is not an object are 400 `invalid_pipeline` and store nothing.
- AC-2: A processor OpenSearch ships and this node does not build, a non-empty `phase_results_processors` and `ignore_failure: true` are 501 `not_implemented` naming the refusal and the supported list, and store nothing.
- AC-3: A mutation names one concrete id; a get selects one or all; a delete of an absent id is 404.
- AC-4: A pipeline is stored, read, listed, replaced and removed over REST.
- AC-5: The routes and the `search_pipeline` parameter exist only with `Config::search_pipelines` on; off, the parameter is 501 `not_implemented` and the routes are 404.
- AC-6: `oversample` multiplies `size` by `sample_factor` (rounded up) before the search runs and `truncate_hits` restores it: a pipeline of both over `size` 3 answers 3 hits while the shard was asked for 6.
- AC-7: `truncate_hits` with its own `target_size` cuts to it; with neither a target nor an earlier `oversample` the search fails 500 `search_pipeline_processing_exception`.
- AC-8: `filter_query` narrows a search with a query and one with none; a `context_prefix` keeps two oversample and truncate pairs apart.
- AC-9: The same pipeline gives the same hits on the single-shard, the sharded and the point-in-time paths.
- AC-10: A named id that is not stored is 400 `pipeline_missing`; `_none` runs nothing and reads nothing.
- AC-11: A read of an unchanged pipeline set is one marker GET and never a LIST; a search that names no pipeline makes no request for the set and runs no pipeline code (the meter shows requests equal to the same search with the switch off).
- AC-12: `index.search.default_pipeline` and a body `search_pipeline` are still refused 501.
- AC-13: Search latency without a named pipeline does not regress: the bench of the single-shard search path before and after differs by under 5%, provisional on this host.

## Test plan

Each AC has a test written first and seen red: AC-1 to AC-3 in `crates/reader/tests/search_pipeline.rs` and `crates/coordinator/tests/search_pipeline_admin.rs` (the engine and admin decisions as pure functions); AC-4, AC-5, AC-10, AC-11, AC-12 in `crates/node/tests/server/search_pipeline_route.rs` over a real server and the in-process store with the request meter; AC-6 to AC-9 in the same file through `_search` against indexed documents. AC-13 is a recorded measurement, not a test, per the `rust-performance` skill.

## S3 request budget

- A put is the doc-set write budget (as [template-routes](../template-routes/SPEC.md)); a get of an unchanged set is one marker GET; a changed set adds one GET per changed pipeline, at most 1,000. No LIST.
- A search naming a pipeline adds the same read, once, through the node's snapshot cache, so a warm node adds one marker GET; the pipeline is parsed per request from the stored bytes (at most 1 MiB). A search naming none adds no request: budget rows `search_pipeline.resolve.requests` and `search_pipeline.none.requests` in [request-budgets](../../request-budgets.md).
- Memory: the read's two buffers (`MAX_DOC_BYTES` each, 2 MiB in all) are reserved from the request-body pool before the read; the parsed pipeline is bounded by its 1 MiB body.

## Switch

`Config::search_pipelines` (`OSS_SEARCH_PIPELINES`), default off, as ingest pipelines are: a stored pipeline is configuration this node runs only on request, so there is no cost when unused, but the allowlist is narrower than OpenSearch's, and turning it on is the owner's statement that a refusal by name is acceptable. Flip when the processors cover what the first tenants store.

## Risks

- A search naming a pipeline after the set changed reads the marker and then the changed pipelines before its own round trips, so it can pass the three-trip budget; a warm node reads one marker. It is opt-in per request. The compiled pipeline is not cached (parsed per search from at most 1 MiB), and a `filter_query` compile parses its query through an unreserved 1 MiB-limited recorder: a known gap, bounded per search.
- The allowlist is a fraction of OpenSearch's processors; a pipeline written for the full set is refused by name, never stored half-run.
- `oversample` raises the work of every shard by the factor; the window check after it stops a factor that pushes `from + size` past 10,000.
- The engine is not core's: a divergence from OpenSearch in `filter_query` combining or rounding shows as a differing hit list. AC-6 and AC-8 pin the Java's behaviour from its source.
