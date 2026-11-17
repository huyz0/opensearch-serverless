# Reader: stored-fields prefetch

## Serves

Parity task R10.4 (hit features), last part; the request budget's round-trip depth (one round to fetch the rows ranked); ADR 0005 (the engine batches announced ranges).

## Change

Before it writes the first hit, the response announces every hit's stored-fields chunk to the engine, a segment at a time (`StoredFields.prefetch` of OpenSearch's fetch phase, `crates/reader/src/response.rs`). The chunks a cold segment must read arrive as one merged batch, in one round trip, instead of one call a chunk, one after another. A segment with no hit announces nothing; a segment held in memory or mapped makes no call.

## Acceptance criteria

- AC-1: the fetch of 30 hits spread over a cold 4,000-document segment, with 4 KiB pages, makes one ranged read in one round trip. Without the announcement it made three reads in three round trips.
- AC-2: the response is unchanged: every step of the compat replay and every response test still pass.

## Test plan

- AC-1: `response::hits_tests::the_hits_of_a_cold_shard_are_fetched_in_one_round`.
- AC-2: the replay (`crates/compat/tests/node_search.rs`) and the reader's tests.

## S3 request budget

The requests are the same bytes read in fewer calls: a merged range replaces ranges one after another. Never more calls than before; depth falls from the number of chunks the hits touch to one. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

One `i32` a hit of the segment, at most the result window (10,000 hits), freed after each segment's announcement.
