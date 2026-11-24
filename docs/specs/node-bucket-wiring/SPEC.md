# Node bucket wiring: heads and publishes of a bucket member

## Serves

R11. Wires [bucket-heads](../bucket-heads/SPEC.md), [bucket-descriptor-heads](../bucket-descriptor-heads/SPEC.md) and [bucket-member-register](../bucket-member-register/SPEC.md) into a node, behind `serverless.buckets.enabled` (default off), under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). With the switch on, a shard whose index descriptor names a bucket is owned through the bucket's head and publishes its commit as its entry in the bucket's manifest. With it off, or for an index whose descriptor names no bucket, nothing changes.

## Change

- `NodeConfig.buckets_enabled`, read from `OSS_BUCKETS_ENABLED` (a flag, default `false`), the node's `serverless.buckets.enabled`.
- `oss_node::writer::BucketWiring`: the node's `DescriptorBucketHeads` over the descriptor store, and the `BucketCommits` its publishers record members in. `BucketWiring::heads()` is what the shard head store routes through; `BucketWiring::register(index, shard, uuid)` is the member register of a shard whose descriptor names a bucket, none otherwise.
- `Publishing::with_buckets(wiring)`: a publisher made for a member shard records its commit through its member register, decided once when the shard's publisher is made.
- Boot builds the wiring only when the switch is on, gives the shard head store its `heads()`, and hands the wiring to the writer role's publishing.

Not here: an index joining a bucket when created (so no index is a member yet), readers reading a member's commit from its bucket manifest, registering a node's open shards with the descriptor-backed heads (`others_open_on` answers false, so a member's release gives the bucket head back with each member; its next write re-takes it), the `serverless.bucket` policy settings in the create path.

## Acceptance criteria

- AC-1: `OSS_BUCKETS_ENABLED` defaults to false, reads `true`/`false`, and refuses anything else.
- AC-2: `BucketWiring::register` is a register for a shard of an index whose descriptor names a bucket and none for an index that names none or has no descriptor.
- AC-3: a writer role publishing with a wiring records a member shard's commit in its bucket entry (one blob and one bucket manifest swap), and never writes the shard's own register.
- AC-4: with no wiring, or for a shard of an unbucketed index, a publish is a blob and a swap of the shard's own register, as before.
- AC-6: with the switch on, the node tells the bucket heads which shards its writer role holds open, so a member's release keeps the bucket head while a sibling member is open on the node.
- AC-7: with the switch on, creating a one-shard index places it in its group's open bucket and its descriptor names that bucket; an index of more than one shard, or any index with the switch off, names none. A create that fails after joining takes the index back out of the bucket.
- AC-8: a reader opening a bucketed index reads its descriptor and its entry in the bucket manifest, two GETs, and resolves the member's commit as it would a shard register; shard 0 only; a member with no commit yet is refused as not followable.
- AC-5: a node booted with the switch off builds no wiring and its shard head store has no bucket routing (existing boot tests unchanged).

## Tests

`crates/node/src/boot/config/tests.rs` (AC-1), `crates/node/tests/roles/bucket_wiring.rs` (AC-2 to AC-4, AC-6, AC-8), `crates/node/tests/server/index_route.rs` (AC-7); AC-5 is the existing boot and role tests.

## S3 request budget

With the switch off: unchanged. On: a member's head operations and publish are those of [bucket-heads](../bucket-heads/SPEC.md) and [bucket-member-register](../bucket-member-register/SPEC.md); the descriptor read that decides membership is one per index per hour. 0 LIST.
- AC-9: a create accepts `index.serverless.bucket_group` (the group the index joins with, else its name) and `index.serverless.bucket` (`off` joins none, `auto` as unset, any other value 400); other create settings are still refused.

## Risks

- A create joins the bucket before its descriptor is written. A create that fails takes itself back out; a node that dies between the join and the descriptor write leaves an unused member entry (never served, since no descriptor names it), and nothing clears it yet.
- The switch stays off until a differential check against a shell-written bucket (the R7 suite) has passed: placement, heads and the member entry layout are checked against this repo's own fixtures only.
