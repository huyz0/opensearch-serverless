# Reader: `_search`

## Serves

Roadmap stage 2, its gate: invariants I-4 (an idle index makes no requests) and I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory, streaming); ADR 0002 (lucene-rust as a pinned git dependency).

## Change

`oss-reader` parses a search body in a stream through the token recorder (never a whole-body JSON parse), ported from OpenSearch 3.10.0: `match_all`, `term`, `match` (standard analyzer), `bool` and `range` (numeric and date), `size`, `from` and `sort` by `_score`, `_doc` or a field. It builds lucene-rust queries, runs them over the open shard, and writes the response as each hit's `_id` and `_source` are fetched. `oss-node` serves `GET|POST /{index}/_search` behind the `reader` feature, opening an index on its first search and keeping it open.

Not in this change: other queries and search keys (refused as OpenSearch refuses unknown ones, or with "not supported by this node yet"), aggregations, date math, ranges on strings, indexes of more than one shard, and picking up a newer commit (the node is told of one in a later change). Each difference is listed in [compatibility.md](../../compatibility.md).

## Acceptance criteria

- AC-1: every `_search` step of the recorded fixtures (`fixtures/compat/search/*.json`, at least 50 steps) gets the status and body OpenSearch 3.10.0 gave, replayed against a running reader node over the recorded shell blobs.
- AC-2: an index opens on its first search with exactly two GETs (descriptor and commit) and no LIST, HEAD or write; a second search makes no request.
- AC-3: an open, idle index makes no request over 300 ms (I-4).
- AC-4: a missing index is `index_not_found_exception` (404) after one GET, and is not kept open.
- AC-5: a malformed or unknown body, or a bad `size` parameter, is refused (400) before any request.
- AC-6: a shard failure names the index, shard 0 and the node id, in OpenSearch's `failed_shards` shape.
- AC-7: `from + size` above 10,000 is refused with OpenSearch's "Result window is too large" error; the hits a search keeps (no more than the window or the shard's documents, a keyword sort value counted at Lucene's longest term) are reserved from the request-body pool before it runs, and each hit's `_id` and `_source` buffers grow only by what the pool grants.
- AC-8: the response is streamed (chunked), one hit's stored fields held at a time.
- AC-9: under eight concurrent clients, four sending an 8 MiB body and four asking for the whole window, the heap peak stays within four recordings at their limit, doubled, plus 2 MiB, under the request-body pool, and under half the bytes sent.
- AC-10: an index that cannot be served (more than one shard, a corrupt descriptor) is refused (400, 500) and not kept; opening past the open-index limit closes one, and concurrent first searches of one index close no other (the limit is a pool since [reader-budgets](../reader-budgets/SPEC.md)).
- AC-11: an index's first search makes exactly the ranged GETs its files need once each (4 for the `match_all` fixture), and none after.

## Test plan

Unit tests beside each parser and builder (`dsl/*_tests.rs`, `lucene_query_tests.rs`, `date.rs`, `numbers.rs`, `response.rs`); node integration tests in `crates/node/tests/it/search.rs` with exact meter counts; the peak test in `crates/node/tests/peak_memory.rs`, its own binary because the counting allocator is process-wide; the replay in `crates/compat/tests/node_search.rs`. The replay was seen red on two error columns before its normalisation, and the peak test red with the recording limit raised to 64 MiB.

## S3 request budget

First search of an index: 2 GET (registers), then ranged GETs of the files the engine reads, through the page cache. Later searches: GETs only for pages not yet cached. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Switch

The `reader` cargo feature of `oss-node`; no runtime setting, since there was no search before.

## Risks

A key order or float format that differs from Jackson's would fail the replay. A recording limit that is too small refuses large legitimate bodies (a term value over the limit is a 400, as the peak test shows). An index never told of a newer commit serves stale hits. A keyword-sorted window is reserved at 32 KiB a value, so a large one on a large index gets a 429 well before it would use that memory. Open indexes were capped by count and closed arbitrarily; [reader-budgets](../reader-budgets/SPEC.md) charges them to a pool and closes the least recently searched.
