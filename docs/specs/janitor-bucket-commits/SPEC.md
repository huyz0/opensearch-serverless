# Janitor judges a bucket member's commit in its bucket's manifest

## Serves

R11, a follow-up of [bucket-commits](../bucket-commits/SPEC.md) and [janitor-node-log](../janitor-node-log/SPEC.md). A bucketed one-shard index publishes its commit as its entry in `buckets/<id>/manifest`, not in `segments/<index>#<uuid>#0/manifest`. The janitor's `Commits` seam read only the second, so a dead node's log for a member always judged `Absent` and was never collected.

## Change

- `oss_janitor::node_log::Commits::committed` takes the index's bucket (`Option<&str>`, from its descriptor, which the collector already read).
- `oss_writer::takeover::commit_checkpoint_of` gives the local checkpoint of commit bytes already read; `commit_checkpoint` reads the shard register and calls it.
- `oss_writer::takeover::member_checkpoint` reads the member's entry with `BucketStore::read_member` and gives the same answer: `None` for a bucket with no register, a non-member, or a member with no commit yet.
- `StoreCommits` routes a bucketed index to the second and any other to the first. Its bucket reads use the bucket store over the same store and base.

## Acceptance criteria

- AC-1: for a bucket member whose entry holds a commit recording checkpoint c, `StoreCommits::committed` gives `Commit(Some(c))`, and for a bucket with no register, a non-member, or a member with no commit yet, `Absent`.
- AC-2: the answer for a member costs one register read of the bucket and one ranged GET of `segments_N`, 0 LIST, and reads no per-shard register.
- AC-3: an index with no bucket is judged exactly as before (its shard register), at the same request count.
- AC-4: the collector, given a dead node's log for a bucket member whose entry's checkpoint covers the log, collects it; given one it does not cover, keeps it.

## S3 request budget

Per member judged: one GET of the bucket register and one ranged GET of the commit's `segments_N`, replacing the shard register GET a plain index costs (same count). Only dead nodes' logs are judged; idle indices make zero. 0 LIST.
