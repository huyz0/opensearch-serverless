# Bucket heads: a bucket's members are owned through one head

## Serves

R11. Ported from the shell's `BucketHeads` and the member paths of `ShardHeadStore` (`ref`, `HeadRef.shown`/`stored`, `bucketHeadIndex`, `read` and `acquire` of a member, `acquireMemberInTurn`) at huyz0/OpenSearch `429b4cbc`, under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md) and on [shard-head](../shard-head/SPEC.md). Every member of a bucket is owned through the one bucket head, so a node that already holds the bucket takes a second member with no write, and a takeover moves the whole bucket with one compare-and-swap.

## Change

- `oss_cluster::heads::BucketHeads`: `bucket_of(index, shard, index_uuid)` answers the `Member { bucket_id, index_uuid }` a shard belongs to, or none for a shard owned through its own head; it must answer the same on every node, which is why an implementation reads the index's descriptor. `Member::new` refuses a bucket id that is not 1 to 64 of `a-z`, `0-9`, `-`, `_`.
- `ShardHeadStore::with_bucket_heads(heads)`: routes members to their bucket's head; unset, every shard is owned through its own head, as before.
- The bucket head is the shard head `#bucket#<bucket id>` shard 0, at `shard_head_key` of that: a name no index can have. It carries the bucket's identity and no incarnation.
- `read` of a member is one GET of the bucket head, shown as the member's: the member's index name and shard, the member's index uuid, the bucket's term, owner, lease and writers.
- `acquire` of a member, one take of a bucket at a time on this node, so members opening in parallel do not each race the one head: an absent head is created at term 1 and won; a head this node already owns (same node id, and the same incarnation or none recorded) is joined at the head's term, with no write; another owner that is live refuses with the head shown as the member's; a dead or unowned head is taken at the next term naming the previous owner as the newest writer. A lost race answers the winner's head.
- Not here: giving a member back (`releaseMember`, `othersOpenOn`), renew, the publish path's use of a bucket head, the descriptor-backed `BucketHeads`, node wiring. Nothing in a node sets `with_bucket_heads` yet.

## Acceptance criteria

- AC-1: with no `BucketHeads` set, or one that answers none, `read` and `acquire` are as before.
- AC-2: `read` of a member is one GET of `shards/<hash>-#bucket#<id>#0`, none when it was never taken, and a head shown as the member's with the bucket's term.
- AC-3: the first member taken creates the bucket head at term 1 with one read and one create; the stored head is named `#bucket#<id>`, shard 0, with no index uuid; the answer is the member's head.
- AC-4: a second member of the bucket taken by the same node joins at the same term with one GET and no write, and so does one taken by the same node id with no incarnation recorded.
- AC-5: a member of a bucket whose owner is live is refused with the owner's head shown as the member's, and nothing is written.
- AC-6: a member of a bucket whose owner is gone takes the head at term + 1, naming the old owner as `taken_from` and the newest writer.
- AC-7: sixteen members of one bucket taken at once on one node are one create and fifteen joins, never a lost race.
- AC-8: two nodes taking one absent bucket head: exactly one wins, and the other is shown the winner's head.
- AC-9: `Member::new` accepts `s02-37` and refuses an empty id, 65 characters, an uppercase letter and a `/`.

## Tests

`crates/cluster/tests/leases/bucket_heads.rs`, on the emulator with the simulated clock and exact request counts, beside the shard-head tests.

## S3 request budget

A member `read`: 1 GET. A member `acquire`: 1 GET to join a bucket this node holds; 1 GET and 1 create for the first member; 1 GET and 1 conditional PUT for a takeover; a lost race adds 1 GET. 0 LIST. Taking the second to Nth member of a bucket costs the bucket's one GET each and no write.

## Risks

- A member joins at the head's term and shares its fate: a lost bucket head fences every member, as intended. The turn per bucket is an in-process mutex, bounded by the buckets this node takes, and is not a correctness lock: the head's compare-and-swap still decides ownership.
- The descriptor-backed `bucket_of` is not here, so the routing is checked against a test map; the R7 differential suite is the first check against a shell-written bucket.
