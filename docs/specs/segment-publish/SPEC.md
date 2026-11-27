# Segment publish: a shard's commit into the object store

## Serves

Parity plan R6.4 (roadmap stage 3, [roadmap](../../roadmap.md)), invariants I-6 (one immutable blob per segment) and the fencing rule (conditional create and register swap, never a lock). Builds on the [shard engine](../shard-engine/SPEC.md); the formats are [blob formats](../blob-formats/SPEC.md), written here as they are read there.

## Change

- `oss-format` gains the writers of what it reads, ported from `SegmentBlobs`, `FileLocation` and `CommitManifest` (huyz0/OpenSearch `feature/serverlessshell`): `segment_blob::encode_header` (a header settled as the Java settles it, files smallest first), `FileLocation::encode`, and `manifest::encode` (term, writer, files, lengths, `wal_ordinal` and `stage_seq` when set, in the Java's field order, names sorted so the same commit is the same bytes).
- `oss-writer` gains `publish` (`crates/writer/src/publish/`): `ShardPublisher`, ported from `SegmentPublisher.publishCommit` for the default settings (one blob per segment in the shard's term container, the register written on every publish):
  - **what is uploaded**: only the files the last published commit does not already name (a segment's files never change once written). A segment's new files are one blob, `<segment>.seg` under `t=<term>/`, created only if absent: when the name is taken (an earlier attempt of the term, or later files of an already published segment such as deletions) the blob is `<segment>-<id>.seg`. The commit point `segments_N` rides in the last segment's blob, so a refresh that adds one segment is one PUT. A file of no length is a blob of its own. Files are streamed through one buffer, never held whole.
  - **fencing**: a manifest of a newer term in the register is a stale writer (`StaleWriter`); the same term from another writer is `ForeignWriter` (the shard-head swap should make it impossible, and a commit assembled from two nodes' segments is worse than a refusal); the swap is conditioned on the generation this publisher last wrote, so a publish after the first reads no register; a swap that fails re-reads once and publishes over what it finds, and the same term and writer found there is this node's own concurrent publish, whose manifest is the answer.
  - **commit source**: a trait over the commit's files (name, length, bytes at an offset), with an implementation over a Lucene `Directory`; `index::Shard::committed_files` names the files of its last commit, to be called after `refresh`.
- Not changed, by design and listed: staged commits and checkpoints (`CommitStaging`), node packs, bucket manifests, the pruning digest (a manifest without one is valid, readers skip it), and the push of a commit to readers (R6.5). The mapping-growth register swap and refresh scheduling from the indexing buffer follow in this milestone's next PRs.

## Acceptance criteria

- AC-1: a segment blob header the Java wrote (`fixtures/shell-blobs`) is re-encoded byte for byte from its parsed files, and headers whose offsets change digit count settle; refused inputs (no files, more than 1,024, a duplicate) are `Err`.
- AC-2: a manifest the Java wrote re-encodes to a manifest that parses to the same term, writer, files and lengths; locations of each form encode to the text they parse from.
- AC-3: publishing a refresh of one new segment is one blob PUT and one register swap; the second and later publishes read nothing; no LIST and no HEAD.
- AC-4: a second publish uploads only the files the first did not name, and the manifest keeps every inherited location.
- AC-5: a blob name already taken makes the blob `<segment>-<id>.seg` and the manifest names it; the taken blob is not overwritten.
- AC-6: a register holding a newer term refuses the publish with `StaleWriter` and writes no register; the same term from another writer is `ForeignWriter`.
- AC-7: a swap that loses re-reads once and publishes over the register; the same term and writer found there answers with that manifest.
- AC-8: a 64 MiB segment publishes with the publisher's peak heap below 2 MiB over the commit's own bytes (counting allocator).
- AC-9: documents indexed through `index::Shard` and published are found by the reader from the object store: `open_shard` over the written register opens the commit and counts the documents, including after a second publish that deletes and updates (the reader opens through the soft-deletes wrapper, [reader-soft-deletes](../reader-soft-deletes/SPEC.md)).

## Test plan

`crates/format/tests/writers.rs` re-encodes the Java's headers and manifests (AC-1, AC-2); `crates/writer/tests/it/publish.rs` over the in-process emulator with a request meter (AC-3 to AC-7); `crates/writer/tests/publish_memory.rs` under the counting allocator (AC-8); `crates/node/tests/roles/publish_read.rs`, which has both roles, indexes through the shard, publishes, and counts through the reader (AC-9).

## S3 request budget

One publish of one new segment: 1 PUT (the segment blob, created if absent) and 1 register swap (a conditional PUT); 0 GET after the first publish; 0 LIST. The first publish adds one register read. A publish that adds `n` segments is `n` blob PUTs and one swap. Nothing scales with documents, indexes or time.

## Switch

None: the register and blob layout are the Java's, and a publish is what a refresh already did locally. Staging and node packs are the Java's own settings and come with their pieces.

## Risks

- A blob or manifest the Java cannot open: AC-1 and AC-2 pin the bytes to the Java's, and the exit gate opens a Rust-written commit with a Java reader.
- Deleting a file the publish is still reading: `Shard` is single-threaded and the publish runs between writes, so no merge or deletion runs underneath; a caller that publishes from another thread must hold the commit (`hold_commit`).
