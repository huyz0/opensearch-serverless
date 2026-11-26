# Parent bit sets are reserved from the memory budget

## Serves

Invariant "bounded memory" ([ADR 0004](../../adr/0004-bounded-memory-and-streaming.md): reserve before allocating); follow-up of [nested-fields](../nested-fields/SPEC.md) and ADR 0067, which left the reader's parent bit sets unmetered.

## Change

Every nested join a search runs builds a parent bit set: the engine's `QueryBitSetProducer` marks the root documents of a segment in one bit a Lucene document, per join node and per segment, lazily and cached for the search. `hits_bytes` (what each search reserves before it runs) now adds `max_doc / 8 + 8 bytes a segment` for each join node the request holds:

- a `nested` query, wherever it sits (in `query`, in `post_filter`, in a `filters` or `filter` aggregation, and inside `bool`, `constant_score`, `dis_max`, `function_score` and another `nested`), plus one for its `inner_hits`;
- a `nested` or `reverse_nested` aggregation;
- a sort key with a `nested` object.

A request with no nested join reserves exactly what it did. An index with no nested object holds no join, so it is untouched.

A `nested` or `reverse_nested` aggregation also makes the engine list a segment's parent and child documents as `i32`s, not a bit set. Each such node adds `4 * max_doc` bytes: its parents and children are distinct documents, so together at most every document once. A node in a query or a sort lists nothing.

## Acceptance criteria

- AC-1: a request with no nested join reserves what it did before (no change in `hits_bytes`).
- AC-2: each `nested` query node adds one bit set's bytes, in `query`, in `post_filter`, in a filter aggregation, and beneath `bool`, `constant_score`, `dis_max`, `function_score` and `nested`; its `inner_hits` adds another.
- AC-3: each `nested` or `reverse_nested` aggregation (also beneath another aggregation) and each nested sort key adds one.
- AC-4: the bytes are `max_doc / 8 + 8 a segment` a node, and a search whose reservation does not fit the pool is refused as any search is.

- AC-5: each `nested` or `reverse_nested` aggregation (also beneath another aggregation) adds `4 * max_doc` bytes of lists to `hits_bytes`, besides its bit set (AC-3); a query or sort join adds none.

## Test plan

- AC-1 to AC-5 (sizes): `crates/reader/src/search_nested_bytes_tests.rs`, over the testkit's nested index.

## S3 request budget

Unchanged: memory only. LIST 0, HEAD 0, PUT 0 added.

## Switch

None: it changes how much a nested search reserves, and no search without a nested join.

## Risks

- The list figure is a bound (every document once), not the engine's measured allocation; a segment of mostly parents and children of one path holds about that. Pending children of a block (at most the window's children times buckets) are not counted.
- The figure is the bit set's own words, from its size; the engine's real allocation was not measured here.
- A search that held many nested nodes on a large shard reserves more than before and can now be refused where it ran: that is the point.
