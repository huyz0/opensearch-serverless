# Reader: the top_hits aggregation

## Serves

Roadmap R10.3, the reader half, last metric: `top_hits` at the top level of a reader's single-shard `_search`, with shard-level hits a coordinator can merge (I-13, REST matches OpenSearch's recordings; ADR 0004, bounded memory; the fetch budget of the `s3-cost` skill).

## Change

`oss-reader` parses `top_hits` as `TopHitsAggregationBuilder.parse` does at OpenSearch 3.10.0: `from`, `size` (default 3), `sort` (the search's own parser), `_source`, `docvalue_fields`, `explain` and `highlight`, with its "Unknown key for a .. in [..]" and "[size] must be greater than or equal to 0" errors. On the shard it checks what `TopHitsAggregatorFactory` and `TopHitsAggregator` check: `from + size` within `index.max_inner_result_window` (100), a `size` of zero (which Lucene's collectors refuse), and the sort.

It collects the query's top documents in its own sort, whatever the request's own sort, `from`, `search_after` and `track_total_hits` are: the total is always exact, and a score sort reports the best score of the window before `from` is skipped (`max_score` is null when sorted, `track_scores` not being served). Each hit is fetched as a hit of the response is, from the same code, and the response announces the stored-field chunks of every list together, so the hits of the response and of all its `top_hits` are one fetch round on a cold shard. The result is the shard-level hits an `InternalTopHits` holds, in request order with the other aggregations; reducing several shards' is the coordinator's (R7), which has to fetch the hits it keeps.

Not in this change: `track_scores`, `version`, `seq_no_primary_term`, `stored_fields`, `script_fields` and `fields` (each refused as "the [top_hits] aggregation's [..] is not supported by this node yet"), a `docvalue_fields` format (as the search), `top_hits` inside a bucket aggregation (no bucket aggregation is served), and the cross-shard reduce.

## Acceptance criteria

- AC-1: the recorded `top_hits` searches (`fixtures/compat/search/paging-sort.json`, `match.json`: the defaults, a scored query with `from`, `from` past the matches, no match, a sort with a source filter and doc values, a keyword sort then the score, `_doc` order, the request's own sort, `from` and total ignored, beside other aggregations, `_source` includes and excludes, numbers as strings, `size` 100, `highlight`, doc values of a date and a number, `explain`) get the status and body OpenSearch 3.10.0 gave, replayed against a reader node.
- AC-2: the recorded `top_hits` errors (`fixtures/compat/search/errors.json`: past the inner result window by size and by from + size, size zero, a negative size and from, an unknown key, a null, an unknown object and array, a non-object, a size that is not a number) get OpenSearch's status and body.
- AC-3: on an index of four segments, a `top_hits` answers the first hits of the score order or of its sort across segments, an exact total of the query's matches, no hits and no best score past the matches, and the scores null when sorted.
- AC-4: a search reserves each `top_hits`' window of hits (at most 100, with a keyword sort's terms) beside the response's own, and the response writes the hits of every list through one set of fetch buffers.
- AC-5: the hits of the response and of two `top_hits` on a cold wide shard are fetched in one round.

## Test plan

`crates/compat/tests/node_search.rs` replays AC-1 and AC-2. `aggs/metric/top_hits_tests.rs` holds AC-3 and the reservation of AC-4 with the parsing and the refusals; `response_hits_tests.rs` holds the writing, the shared buffers and AC-5.

## S3 request budget

The extra searches read the same doc values and postings the query already reads, from the opened segments; the stored fields of every list are announced together, so a cold shard's fetch stays one round and the request count grows only with the hits' distinct chunks. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with `top_hits` was refused before.

## Risks

Each `top_hits` runs the query once more, so a request with many pays the query's cost for each (OpenSearch runs one pass); the window of 100 bounds the memory. A coordinator's merge needs shard identity the hits do not carry yet.
