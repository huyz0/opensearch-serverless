# The janitor's pass over a writer's shards

## Serves

Parity plan R7.6, node assembly: the janitor crate sweeps one shard at a time, and nothing yet decides which shards a node sweeps or hands the sweep the points in time that pin blobs. `JanitorPass` is that decision for the shards a writer holds. A bucketed member is a one-shard index, so its only shard is shard 0 and is swept as such, which is the shard its views pin ([janitor-point-in-time-pins](../janitor-point-in-time-pins/SPEC.md)).

## Change

`oss-node`, with the `writer` feature, gains `JanitorPass` and `JanitorReport`, and `WriterRole::held_shards`.

- A pass sweeps a held shard only when it published a commit the last sweep did not see, when a blob of it is still being watched for the second sighting, or when its last sweep was pinned whole or found no commit. A held shard nothing happens to makes no request, so the cost scales with publishes and blobs, never with time.
- One pass reads the live points in time once and answers every shard it sweeps from that snapshot.
- The pass reserves one buffer from the request-body pool before it allocates it; a pool without room skips the pass and reports why.
- Deletion rules, the dry-run switch and the two-sweep grace are the janitor crate's, unchanged: the caller passes the sweep's `Config` each pass so the settings may change.

## Acceptance criteria

- AC-1: a held shard that has published nothing makes no request, and the pass reports nothing.
- AC-2: a blob of an older term that nothing names is kept at the first sweep and deleted at the second, which runs although nothing published; after that the shard is not swept and the pass makes no request.
- AC-3: under a dry run the blob is counted as would-delete and kept.
- AC-4: a blob a live point in time names is kept.
- AC-5: a new published commit makes the shard due again.
- AC-6: a pass with no memory for its buffer sweeps nothing and says so.
- AC-7: a shard pinned whole by a capture is reported pinned, is swept again at the next pass once the capture is gone, and then follows AC-2.

## Test plan

`crates/node/tests/writer_janitor.rs` over the emulator and a real writer role.

## S3 request budget

A shard swept costs what the janitor crate's sweep costs: a manifest read, LIST pages of the shard's storage path, and deletes. The pass adds one listing of the points in time for the whole pass, only on a pass that sweeps. A pass with nothing due makes none.

## Risks

- A shard whose commit was published before this node took it is swept only after this node publishes it again; its older blobs wait until then.
