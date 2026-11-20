# A node with the reader and writer roles serves a usable index

## Serves

Node assembly: one assembled node creates an index, takes a bulk, refreshes and searches it with the scores stock OpenSearch gave. `WriterRole::refresh` publishes the commit that holds what it refreshed, so a search that follows sees the writes.

## Change

`Writer` (the assembly's writer input) gains `names` (the shard names the committer's admission shares with the role) and `incarnation` (the index-incarnation check). `WriterRole::refresh` refreshes the engine and, where the role publishes, publishes the shard's commit before it returns and records it as published, so the publish timer does not repeat it. `crates/compat` replays every search fixture against an assembled node over the in-process emulator.

## Acceptance criteria

- AC-1: a refresh on a role that publishes makes the shard's commit available (`newest_commit`) and the next publish pass finds nothing due.
- AC-2: a refresh of a shard the role does not hold is refused.
- AC-3: an assembled node answers every recorded search fixture (index created by PUT, documents by `_bulk`, `_refresh`, then searches) with the recorded hits and scores, apart from the documented differences.

## Test plan

`crates/node/tests/writer_publish.rs` (AC-1, AC-2); `crates/compat/tests/node_cluster.rs` (AC-3).

## S3 request budget

A refresh with publishing costs one blob PUT and one register swap (the publish), as a timer pass does. LIST 0.

## Risks

- The create-index acknowledgement carries a `shards` field stock OpenSearch's does not (documented); the replay removes it, and removes the column offsets of parse errors, which the recordings carry from the stock node's different framing.
