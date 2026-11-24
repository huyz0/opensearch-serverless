# A read of an index that never held a document

## Serves

Compatibility: stock OpenSearch answers `_search` and `_count` of an index that was created and never written with 200 and no hits. A reader node answered 404 `index_not_found_exception` until the index had a published commit. Found replaying the security access fixture (step `search allowed`).

## Change

`oss-reader` opens a shard whose descriptor exists and whose commit does not (the shard's commit register is absent, or a bucketed index's manifest is absent or holds no commit for it) as a shard of no documents. `ShardFiles` then holds one file, the engine's own empty `segments_N`, served from memory (`empty_commit`); the prefetch plan skips it. Every read route that opens a shard (`_search`, `_count`, `_msearch`, `_field_caps`, `_validate/query`, `_analyze` on a field, scrolls and points in time over it) sees an index of no documents. An index with no descriptor, a tombstone, and an alias are unchanged.

Not here: the writer's overlay is unchanged; a document accepted and not yet published is found as before.

## Acceptance criteria

- AC-1: `POST /{index}/_search` of a created, never-written index answers 200 with `hits.total` 0 `eq`, `max_score` null, no hits and `_shards` of 1 total, 1 successful.
- AC-2: `_count` of it answers `{"count":0,...}`; a `value_count` aggregation answers 0.
- AC-3: a search of a name that has no descriptor still answers 404 `index_not_found_exception`.
- AC-4: opening it makes no LIST, HEAD, PUT, COPY or DELETE and at most the descriptor's and the register's GETs; a second search makes none.
- AC-5: a bucketed index whose bucket manifest is absent, or holds no commit for it, opens the same way; one that is not a member of its bucket is still refused.

## Test plan

`crates/reader/tests/it/never_written.rs` (the open and its request counts, plain and bucketed) and `crates/node/tests/it/never_written.rs` (HTTP). `crates/reader/tests/it/open_bucket.rs` pinned "no commit yet is not followable"; that half now asserts the empty commit.

## S3 request budget

Unchanged: the descriptor GET and the register GET, which is a 404 here, as it was. No LIST. Once open, the empty shard is held and charged like any other.

## Switch

None: a refusal that stock does not make becomes the stock answer, with no cost.

## Risks

- A commit register that is missing for another reason (a deleted blob) now reads as an empty index instead of a 404. The janitor deletes registers only with their index's descriptor tombstone.
