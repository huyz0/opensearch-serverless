# Reader budgets: open indexes charged to memory, least recently used closed first

## Serves

The reader's deferred memory gaps ([reader-search](../reader-search/SPEC.md)): open indexes were capped by count, eviction was arbitrary, and no test filled a real 10,000-hit window. [ADR 0004](../../adr/0004-bounded-memory-and-streaming.md), [ADR 0014](../../adr/0014-open-indexes-pool.md), invariants I-15 to I-17.

## Change

- `oss-testkit` gains `index` (feature `index`): a one-shard index written with lucene-rust's `IndexWriter` and laid out as the shell's blobs (descriptor, commit manifest, one blob a file), with any number of documents, segments, keyword and text fields.
- `oss-mem` gains the pool `open_indexes`.
- `oss-reader` splits opening into locating (the two register reads) and opening, and estimates what an open index keeps in the heap from what locating read (`open_cost`).
- `oss-node`'s `SearchService` charges each open index to `open_indexes` before opening it, closes the least recently searched index when the pool is full, and answers 429 when nothing is left to close. The count cap (`MAX_OPEN_INDEXES`, `with_max_open`) goes.
- Not changed: what a search reads from the store, the response, or the request-body pool's reservations.

## Acceptance criteria

- AC-1: an index built by `oss_testkit::index` with 12,000 documents in 3 segments is found by a reader node with two GETs, and a search sorted by `n` descending answers documents 11,999 and 11,998 with the `_id`, `_source` and sort value the builder wrote, and `total` 10,000 `gte`, as `SearchPhaseController.TopDocsStats.getTotalHits` caps a count past `track_total_hits`.
- AC-2: a `_search` of size 10,000 sorted by `n` over that index answers 200 with exactly 10,000 hits in `n` order and `total` 10,000 `gte`, and the heap the window takes beyond a one-hit search, measured by the counting allocator with a client that keeps no copy of the reply, stays within what the search reserved for its hits (`hits_bytes`), and the whole peak within that plus 1 MiB; the same search with a request-body pool one byte short of the window, beyond the server's own buffers, answers 429.
- AC-3: for each of 14 built shapes, after the open and a mix of searches (10 to 200,000 documents, 1 to 200 segments, compound and separate files, 0 to 100 keyword or text fields), `open_cost` is at least the heap the open retains, measured by the counting allocator, and at most 8 times it or 128 KiB, whichever is larger.
- AC-4: `open_cost` counts a file whole exactly when the cache reads it whole: a pinned extension, one page or less, or (with no demand paging and no spill) any file; a file over a page that the cache pages is not counted.
- AC-5: an open index holds its charge from `open_indexes` while it is open; an index a search holds is never closed, so a charge that does not fit beside held indexes is refused and closes neither, nor an idle index whose closing could not make room; with every index closed the pool reads 0 in use.
- AC-6: with a pool that holds two of three indexes, searching A, B, A, then C closes B (the least recently searched), so a search of A then makes no request and a search of B makes the two GETs of a cold open.
- AC-7: an index larger than the whole pool answers 429 `circuit_breaking_exception` from the breaker `open_indexes` with durability `PERMANENT`, and the indexes already open stay open; one that only does not fit now answers durability `TRANSIENT`.
- AC-8: concurrent first searches of one index open it once and close nothing else: the others wait for the first open rather than charging the pool for copies.
- AC-9: a reader whose budget gives `open_indexes` nothing does not start.

## Test plan

- AC-1: `a_built_index_is_found_with_two_gets_and_served` (node). Fails if the layout's keys, registers or file lengths are wrong, or a total past 10,000 is reported as counted (it was, until this change: the recorded fixtures never passed 10,000).
- AC-2: `a_full_window_stays_within_its_reservation` (node, its own binary for the counting allocator; the index is built in a child process so the engine's writing is not in the heap peak) and the 429 case in it. Fails if `hits_bytes` under-counts a real window or the response holds hits beyond it: it did, until this change, by 16% (the engine's collector queue and slots, alive beside its collected hits, were not counted).
- AC-3: `the_estimate_bounds_what_an_open_retains` (reader, own binary). Fails if a constant is lowered or a whole-read file is missed.
- AC-4: unit tests of the whole-file rule in `open_cost`.
- AC-5, AC-6, AC-7, AC-8: `oss-node` unit tests over built indexes (`search_tests.rs`), the meter showing which index reopened; `open_indexes.rs` unit tests for the order and the one-open-at-a-time wait; `an_index_the_open_pool_cannot_hold_is_refused_with_429` over HTTP.
- AC-9: `a_node_with_no_open_index_pool_does_not_start`.

## S3 request budget

Unchanged: a cold search is still the two register GETs and the engine's ranged reads, a warm search 0 (`search.cold.*`, `search.warm.requests`). The estimate reads only what the open already read. An evicted index costs a cold open when searched again.

## Switch

None: a count cap is replaced by a byte bound, with no new behaviour to compare. The pool's size is the caller's budget split, as for every pool.

## Risks

- The estimate's constants are measured on one platform and lucene-rust revision; an engine change that keeps more heap would show as AC-3 failing.
- On a host without `userfaultfd` and with no disk tier, every file is read whole; the estimate follows the cache's own rule, so it charges that, and AC-4 covers it.
