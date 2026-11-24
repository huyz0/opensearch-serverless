# Bucket member orphans: members no index owns are swept

## Serves

R11. Closes the second risk in [node-bucket-wiring](../node-bucket-wiring/SPEC.md) and the remainder of [bucket-member-on-delete](../bucket-member-on-delete/SPEC.md): a node that dies between a join and the descriptor write, or between a descriptor delete and its leave, leaves a member that no descriptor names, and nothing took it out, so its bucket filled with dead members. Under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). Runs only with `OSS_BUCKETS_ENABLED` on; otherwise no code path exists.

## Change

- `BucketMemberSweep::slice(now)`: lists the next 4 buckets (cursor kept in memory, wrapping), reads each manifest, reads the descriptor of each member's index name (one GET a distinct member), and calls a member whose index is absent or has another uuid a candidate.
- A candidate is taken out (`BucketWiring::leave`: one GET and one conditional PUT) only at a second sighting at least the grace after the first. An index joining right now has no descriptor yet, so a single sighting must never be enough. A member that stops being a candidate is forgotten. At most 4,096 sightings are kept.
- The orphan-shard sweep's timer runs it after each slice on the node ranked to sweep (so one node sweeps, the others make no request). It honours the janitor dry-run setting: counts, removes nothing. Grace: one hour, a constant.
- `BucketAssigner::members` and `BucketWiring::members` read a bucket's `(uuid, index name)` pairs.

The sightings live in memory: a sweep that moves to another node starts its grace over, which only delays a removal.

## Acceptance criteria

- AC-1: a member whose index is absent or has another uuid is kept at its first sighting and before the grace, and taken out at the first slice at or after the grace; a member whose descriptor names it is never taken out.
- AC-2: a member whose descriptor appears before the grace is kept, and forgotten as a candidate.
- AC-3: under dry run the removals are counted and none is made.
- AC-4: a first sighting writes nothing: one LIST of one key a bucket plus one for the short page, the manifests and descriptors read, 0 PUT.
- AC-5: a slice judges at most 4 buckets, and the cursor wraps after the last.
- AC-6: `BucketAssigner::members` lists a bucket's `(uuid, index name)` pairs, and a bucket with no manifest has none.

## Tests

`crates/node/tests/roles/bucket_orphans.rs` (AC-1 to AC-5), `crates/writer/tests/bucket_assign.rs` (AC-6).

## S3 request budget

Per slice (one a janitor interval on one node): 1 LIST page of at most 4 keys, at most 4 manifest GETs, at most one descriptor GET a member (at most 4,096 a slice, in practice a bucket's members), and per orphan taken out one GET and one PUT. 0 LIST per request; nothing scales with documents or elapsed time per index; idle indexes cost nothing beyond their member's single descriptor GET per cycle. Off when buckets are off.

## Risks

- A grace shorter than a create's gap between join and descriptor write would remove a live joiner's member; the gap is milliseconds and the grace an hour.
- A bucket larger than a slice's budget of descriptor reads is still judged whole (at most 1,024 members a bucket).
