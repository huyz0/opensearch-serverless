# The local checkpoint of a shard's published commit

## Serves

R8 janitor: collecting a dead node's write log needs to know whether the shard's published commit holds everything the log does ([dead logs](../node-dead-logs/SPEC.md)).

## Change

`oss_writer::takeover::commit_checkpoint` reads the shard's commit register, finds the `segments_N` file of the commit in its manifest, reads that one file by ranged GET (bounded at 4 MiB) and returns the `local_checkpoint` in its user data: `None` when the shard has no commit, `Some(None)` when the commit records none.

## Acceptance criteria

- AC-1: a published commit answers the highest sequence number it holds.
- AC-2: a shard with no commit answers `None`.
- AC-3: a read is one register read and one ranged GET: no LIST and no PUT.

## Test plan

`crates/writer/tests/it/commit_checkpoint.rs` over the emulator.

## S3 request budget

One register GET and one ranged GET per shard judged, once per dead node's shard per pass; no LIST, no PUT.

## Risks

A `segments_N` over 4 MiB is refused as too large; a shard that large in segments is far outside the design.
