# Bucket commits: members' commits in their bucket's one manifest

## Serves

R11. Ported from the shell's `BucketCommits` (`swapAll`, `member`, `forget`, `MAX_ATTEMPTS`) and the register half of `BucketStore` (`read`, `swap`, `readMember`, `create`) at huyz0/OpenSearch `429b4cbc`, on the manifest value of [bucket-manifest-writer](../bucket-manifest-writer/SPEC.md) and [bucket-manifest-compact-writer](../bucket-manifest-compact-writer/SPEC.md), under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). One compare-and-swap of the bucket register lands every member's commit that checkpoints together; each member is judged against its own entry's version, not the register's generation, which every sibling's checkpoint moves.

## Change

`oss_writer::bucket::BucketStore` reads a bucket's manifest register (`buckets/<id>/manifest`) with its generation, creates it empty, and swaps it, writing the manifest in the form its `ManifestWrite` names (JSON, compact with a codec, or compact compressed only when that pays; default auto zstd). `BucketCommits` holds a store and, per bucket, a lock and the manifest this node last wrote with its generation:

- `swap_all(bucket, swaps)` takes member uuid to `Swap { expected_version, commit }` and gives each member's outcome: `Applied(new_version)` or `Conflict(Some(version))` when its entry is at another version, or `Conflict(None)` when it is not a member. Members whose version matches advance by one and land together in one swap; the others are left as they are. When none advances nothing is written.
- Writes to one bucket are serialised on this node. The last manifest this node wrote is kept, so the next swap needs no read. A lost swap (another node wrote the bucket) costs one read and a retry against it, up to 16 attempts, then `BucketError::Busy`.
- When every member in a call had moved and the manifest was the cached one, it is dropped and read fresh once before answering, so a stale cache cannot report conflicts forever.
- `member_commit(bucket, uuid)` reads one member's version and commit bytes through the member reader (no whole-manifest parse), for a shard's open; a bucket or member that does not exist is an error.
- `forget(bucket)` drops the cache.

Not here: bucket assignment, the size decider, bucket heads and ownership, settings, the publisher's use of a member register. Nothing in a node calls this yet.

## Acceptance criteria

- AC-1: a cold `swap_all` of three members at their expected versions is one register read and one register write; the manifest then holds each member's new commit at version + 1 and the other members unchanged.
- AC-2: a second `swap_all` on the same bucket makes one register write and no read.
- AC-3: a member whose expected version is not its entry's gets `Conflict(Some(its version))`, its siblings in the same call still land, and the stale member's entry is unchanged; a uuid that is not a member gets `Conflict(None)`; a call where nothing advances writes nothing.
- AC-4: another node's write between two of this node's swaps costs one read and one retry, and a member that the other node did not touch is not a conflict (the sibling's checkpoint moved the register, not this member's entry); a member the other node moved gets `Conflict(Some(new version))`.
- AC-5: a register that is lost 16 times gives `Busy` after 16 writes, not a hang.
- AC-6: a bucket with no register is `BucketError::NoBucket`; `create` of an existing bucket reports it exists and changes nothing.
- AC-7: `member_commit` returns the member's version and exact commit bytes, `None` for a member with no commit yet, and an error for a non-member.
- AC-8: each `ManifestWrite` form writes bytes `BucketManifest::parse` reads back and the member reader reads: JSON starts with `{`, auto zstd writes `OSBM` and compresses a manifest over 32 KiB.
- AC-9: threads swapping different members of one bucket together land every member exactly once, with no more register writes than swaps and no conflict among them.
- AC-10: a commit that is not a commit manifest is refused before any request.

## Tests

`crates/writer/tests/it/bucket_commits.rs`, over the in-process emulator with its request meter.

## S3 request budget

Per `swap_all`: one conditional PUT, plus one GET when this node has not written the bucket since start or since a lost swap; each lost swap adds one GET and one PUT. Members that checkpoint together share the one PUT. 0 LIST. A bucket nobody checkpoints makes no request. Idle members make zero.

## Risks

- The cache assumes this node is the only writer between takeovers, as in the shell; another writer costs one lost swap and a read, never a lost update (the swap is conditional on the generation).
- A whole manifest is rewritten per swap, so the swap's bytes grow with the bucket (up to 1,024 members): the reason bucketing stays behind a setting until measured.
