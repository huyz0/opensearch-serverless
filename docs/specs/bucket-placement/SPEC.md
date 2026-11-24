# Bucket placement: which writer takes a bucket nobody holds

## Serves

R11. Ported from the shell's `BucketPlacement` (`score`, `observe`, `rank`, `ownerFor`, `keeps`, the hold-down constants) at huyz0/OpenSearch `429b4cbc`, under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md) and on [placement](../placement/SPEC.md). A bucket nobody holds goes to the highest-ranked live writer for it, by rendezvous hashing over the writer leases every node already reads: every node computes the same answer from the same leases with no request, and a node joining or leaving moves about one bucket in N. Without it, buckets pile up on whichever nodes clients happen to reach.

## Change

- `oss_writer::bucket::BucketPlacement`: `new(hold_down_millis)`, `observe(leases, now)`, `rank(bucket_id, leases, now, writer)`, `owner_for(..)` and `keeps(bucket_id, owner, leases, now, writer)`. Constants `DEFAULT_HOLD_DOWN_MILLIS` (120,000), `MAX_HOLD_DOWN_MILLIS` (1,800,000), `LOAD_BOUND` (1.25) and `KEEP_RANKS` (2).
- A bucket's score on a node is `h1` of murmur3 x64 128-bit, seed 0, of `<bucket id> 0x00 <node id>` (the same hash as the stripe), compared as signed 64-bit; the highest wins, ties by the Java's string order of the node id.
- Churn: a node is ranked only once seen for the hold-down; the first view this placement has treats everyone in it as settled; a node back under a new incarnation soon after the last (within twice its hold-down) doubles its hold-down, up to 30 minutes, and otherwise gets the base one. Writers in their hold-down are left out while any settled writer is live. A writer holding more than 1.25 times the mean of the candidates' reported loads (a lease reporting none is left out of the mean) is ranked last. Expired, revoked and non-writer leases are never ranked.
- `keeps` is true while the holder ranks in the top two.
- State is the first-seen time per node, one entry per node ever seen, behind a mutex.

Not here: bucket heads and ownership, node wiring. Nothing in a node calls this yet.

## Acceptance criteria

- AC-1: a bucket's score on a node is the murmur3 `h1` of `bucket 0x00 node`, and `rank` orders settled, unloaded writers by it, highest first, with equal scores in the Java's node id order.
- AC-2: expired, revoked and non-writer leases are not ranked; with no live writer the owner is none.
- AC-3: the first view settles every node in it; a node first seen later is left out for the hold-down while a settled writer is live, ranked once it has passed, and ranked when it is the only live writer.
- AC-4: a node back under a new incarnation within twice its hold-down waits twice as long, again doubling to the 30-minute ceiling; after longer it waits the base hold-down; the same incarnation changes nothing; a lease with no incarnation id never restarts one.
- AC-5: a writer whose reported load is over 1.25 times the mean of the candidates' is ranked after every other, whatever its score; leases reporting no load are not in the mean; no mean (nobody reports) overloads nobody.
- AC-6: adding a writer to a fleet of N moves the owner of about 1 in N+1 of 2,000 buckets, and only ever to the new writer.
- AC-7: `keeps` is true for the first two ranked and false for the rest and for a holder not in the ranking.

## Tests

`crates/writer/tests/bucket_placement.rs`, by hand-built leases and the murmur function itself.

## S3 request budget

None: pure computation over the leases a node already holds.

## Risks

- The ranking is checked against the murmur function and the rules above, not against a ranking the Java recorded; the R7 differential suite is the first independent check. A disagreement between nodes costs at most a forwarded write, since the head's compare-and-swap decides ownership.
- The first-seen map holds one entry per node ever seen; fleets are hundreds of nodes, and a restarting node reuses its entry.
