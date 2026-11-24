# Bucket member register: a publisher records its commit in its bucket

## Serves

R11. Ported from the shell's `CommitRegister` (`read`, `compareAndSwap`, `of`), `SegmentPublisher.commitRegister` and `BucketCommits.member` at huyz0/OpenSearch `429b4cbc`, on [bucket-commits](../bucket-commits/SPEC.md) and [segment-publish](../segment-publish/SPEC.md), under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). A publisher reads and swaps its commit only through a `CommitRegister`, so a bucket member and a shard of its own publish alike. The generation is whatever tells this shard's commit apart from the one before: the register's generation for a shard of its own, the member entry's version for a bucket member, so a sibling's swap never looks like this shard's commit moving.

## Change

- `oss_writer::publish::CommitRegister` (`read`, `swap`) and `RegisterValue { generation, bytes }`. The shard's own manifest register is the default and behaves exactly as before: the same reads (a first read with room for a thousand files, one more with room for the most), the same conditional swap.
- `ShardPublisher::commit_register(register)` sets where the commit is recorded, before the first publish.
- `oss_writer::bucket::MemberRegister::new(commits, bucket, index_uuid)`: `read` is the member's latest commit and its version (none before the first); `swap` is a swap of the member's entry in the bucket's manifest at the version read (0 for none), through `BucketCommits::swap_all`, so a sibling's swap is retried inside it and never fails this member's. A lost swap answers the version the entry is at.
- `PublishError::Bucket` carries a bucket failure (no bucket, not a member, a busy register).

Not here: the committer's checkpointing of many members in one swap (staging), the node's choice of register for a bucketed shard, node wiring.

## Acceptance criteria

- AC-1: a publisher with the default register makes the requests it made before: one read of the register for the first publish and none after, one swap a publish.
- AC-2: a bucket member's publish lands in its entry of the bucket manifest: the commit is the manifest the publisher built, the entry's version advances by one, and the shard's own register is never written.
- AC-3: a member's second publish reads nothing: one blob and one bucket manifest swap, over the version it wrote.
- AC-4: a sibling's publish in between does not fail this member's: its swap lands over the version its entry is at, and the sibling's commit is untouched.
- AC-5: a member publisher reads the commit already in its entry before its first publish and builds on its files, so files it already holds are not uploaded again.
- AC-6: a publish at an older term than the member's commit is `PublishError::Stale`, and one by another writer at the same term is `PublishError::Foreign`, as for a shard of its own.
- AC-7: a publisher for an index that is not a member, or a bucket that does not exist, fails with `PublishError::Bucket`.

## Tests

`crates/writer/tests/publish_member.rs` over the emulator with its request meter, and the existing `publish` tests unchanged.

## S3 request budget

A member publish: one blob PUT for its files and one conditional PUT of the bucket manifest; its first publish adds one read of the bucket manifest. Contention inside `swap_all` retries are the bucket-commits budget. 0 LIST. A shard of its own: unchanged.

## Risks

- Each member publish rewrites the whole bucket manifest, up to its 2 MiB bound; checkpointing many members in one swap is the committer's saving and is not here.
- The register trait object adds one indirection a publish.
