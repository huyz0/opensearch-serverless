# 0062: A snapshot is exported to a stock repository by copying the shard files

**Status:** Proposed, awaiting independent review

## Context

[ADR 0060](0060-stock-snapshot-restore-reads-a-source-repository.md) lets a tenant move data in from a stock OpenSearch repository. The way out is the other half of R14.8: a repository a stock 3.10 cluster can restore. The native snapshot cannot be that. It is shallow ([ADR 0042](0042-snapshots-taken-shallow-first.md)): it names the published blobs and copies none, so its repository holds nothing a stock node could read. A stock restore needs the bytes of every Lucene file, the SMILE metadata blobs and the `index-N` generation.

The owner decided stock interop is wanted, opt-in, restore first. Export has no Java counterpart in the shell, so there is nothing to port; the target is the layout the recorded 3.10 repository shows (`fixtures/snapshots/stock/`).

## Decision

**Which request.** `PUT /_snapshot/{repo}/{name}` where `{repo}` is a repository registered with `format: "stock"` and not `readonly` takes a standard, copying snapshot of the named indices into that repository. A stock repository registered `readonly` stays restore-only. A native repository is untouched, and its rows in the request-budget table do not change.

**The target store.** Writing is a stronger permission than reading, so it has its own allow-lists, `OSS_STOCK_EXPORT_FS_ROOTS` and `OSS_STOCK_EXPORT_S3_ROOTS`, empty by default: nothing is writable until an operator names a root. The checks of the read-side allow-lists apply unchanged (canonical path under a root, no symlink, an `s3` target that overlaps this deployment's own bucket and prefix in either direction refused). The target store is a wrapper that allows reads, creates and puts under the repository prefix and answers every delete with an error; a failed take leaves orphan blobs, as a stock take does, which a stock delete collects.

**What is written, in this order.**

1. For each shard, each file of its published commit, copied as a whole-file blob `__<id>` (no chunking below the store's single-object limit; a file above it is refused 501 before any write), streamed through two pooled buffers as the native restore's copy is, with the Lucene footer CRC checked as the bytes pass.
2. Each shard's `snap-<snapshot uuid>.dat` and its `index-<generation>` of shard files.
3. Each index's `meta-<id>.dat` (its mapping and settings as stock `IndexMetadata`).
4. The snapshot's `snap-<uuid>.dat` and the global `meta-<uuid>.dat` (empty metadata: `include_global_state` is refused).
5. `index-N+1`, by **conditional create**: the generation is the fence. A second writer of the same repository at the same generation gets a conflict and fails; its blobs are orphans. Never a lock or lease.
6. `index.latest`, an unconditional put of the new generation, as stock does. A reader that sees the old value reads the old, still valid `index-N`.

The generation to extend is read from `index.latest` and `index-N` (two GETs, no LIST). A repository with neither is a new one and starts at generation 0.

**Formats.** An encoder for SMILE in `oss-format` (the decoder's counterpart, same bounds), the Lucene header and footer writer, and the `RepositoryData`, `SnapshotInfo`, `IndexMetadata` and `BlobStoreIndexShardSnapshot` writers. Blobs are written uncompressed: a stock reader detects compression, so the repository's `compress` setting only governs what stock writes. Every writer is tested against bytes the server's own `*_FORMAT` classes wrote, and against the recorded repository: what the Rust node decodes from a Rust-written blob equals what it decodes from the stock-written one for the same index.

**Consistency.** A shard is copied from one published commit, which is immutable, so the copy is consistent without a quiesce. The protective marker the native take writes ([ADR 0042](0042-snapshots-taken-shallow-first.md)) pins the commit for the copy so the janitor cannot reclaim its blobs, and is released after the finish; a crash leaves the marker to the native recovery.

**No dedupe in the first version.** Stock reuses a file already in the repository when its name, length and checksum match the previous shard generation. The first version copies every file of every snapshot. The cost is bytes, not requests, and it is the opt-in's stated price; a later spec may carry the previous generation. Restoring on stock works either way.

**Refused 501, before any write.** A bucketed or shared-pack index (its files are not whole published blobs of one shard), `include_global_state`, `partial`, a file above the single-object limit, a delete of a stock snapshot, an index whose settings or mapping stock cannot restore.

**No LIST, and request scaling.** Requests scale with files and bytes: per shard one manifest read, a ranged read for each buffer of each file, one put for each file, one for the shard's snapshot blob, one for its generation; per index one put; per snapshot four puts and the two generation reads. Nothing scales with indices that are idle, and nothing is a LIST. The rows are measured and pinned in the budget table.

**Bounds.** The same numbers as the restore: 100 indices, 4,096 shards, 10,000 files a shard, 100,000 file entries in all, 128-byte names, blobs of 16 MiB. The copy reserves two buffers from the memory pool before the first read and answers 429 when it cannot.

**Switch.** `serverless.snapshots.stock.export.enabled` (`OSS_SNAPSHOTS_STOCK_EXPORT_ENABLED`), default off: it copies the tenant's bytes out of the deployment. Both sides are tested: off answers 501 with no request to either store; on exports.

**The proof that it works.** A repository the Rust node wrote is restored by a stock 3.10 node (the `opensearch-min` build) and answers the searches the source index answered. That run is by hand, like the Java mixed-fleet proof, and is recorded in `VERIFIED.md` with the version and result; until it is run the criterion says NOT-RUN.

## Consequences

- Easier: a tenant takes its data out as a stock-restorable repository, from an operator-named directory or bucket.
- Harder: a SMILE encoder and stock writers to keep against OpenSearch 3.x; every snapshot copies all bytes until dedupe lands.
- Rules out for now: dedupe against earlier snapshots, a delete of a stock snapshot, bucketed and shared-pack indices, global state, and a stock repository both read and written by one registration.
