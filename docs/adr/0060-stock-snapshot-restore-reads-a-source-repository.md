# 0060: A stock snapshot is restored by reading its repository through a source store

**Status:** Proposed

## Context

A tenant moves data in from a stock OpenSearch cluster by restoring a snapshot that cluster wrote to an `fs` or `s3` repository (the `BlobStoreRepository` layout of OpenSearch 3.x). The native repository ([ADR 0042](0042-snapshots-taken-shallow-first.md), [ADR 0045](0045-shallow-snapshot-restore.md)) is different in kind: its snapshot lives in this deployment's object store and records which published blobs hold each shard, copying nothing. A stock repository holds copied Lucene files, chunked, under its own root, with metadata written in SMILE, optionally compressed, behind a Lucene checksum header and footer.

Stock reads cost requests: `index.latest`, `index-N`, `snap-<uuid>.dat`, one `meta-<uuid>.dat` an index, one shard generation blob and one `snap-<uuid>.dat` a shard, then the data blobs. Stock itself falls back to a LIST of `index-` when `index.latest` is missing; a LIST is priced like a PUT here.

## Decision

- A repository registered with the setting `format: "stock"` (a key only this node knows) is read as a stock repository. Without it a repository is native, as today, and nothing about the native path changes: the native rows of the request-budget table stay as they are.
- The repository reads through a **source store**: for `fs`, a directory under an allow-list of roots the operator names (`OSS_STOCK_REPO_ROOTS`, empty by default, so no path is readable until one is named; `location` is resolved and must stay under a root); for `s3`, a bucket and `base_path` under the node's own credentials and endpoint. The source store is read-only: it is opened through `oss-blob` like every store and is metered.
- Restore only first; export to a stock repository follows as its own change under the same switch.
- **No LIST on the restore path.** The generation comes from `index.latest`; a repository without it is refused 400 with the reason, because a restore that LISTs would cost a PUT's price on a path that can name its target. (A stock repository on S3 always writes it.)
- The metadata a restore needs (index mapping and settings, in `meta-*.dat`) is SMILE. A sans-IO SMILE decoder to JSON text is added to the document crate, bounded in depth and size, and the decoded JSON goes through the same mapping and settings parsing a create uses, so a stock index is created exactly as the Rust node would create one with that mapping.
- Each shard's files are copied as whole-file blobs into the new index's first-term directory by the existing streaming copy, with a `__<uuid>.partN` chunk set read in order into one file; the manifest is built from the stock file list (name, length, checksum, written_by) and lands as the native restore's does, under the same restore mark, cleanup and refusals.
- Lucene checksum footers of metadata blobs are verified before the bytes are decoded; a data file's recorded length and checksum are recorded in the manifest, and a length that does not match what was read fails the restore (cleaned up as any failure is).
- Behind `serverless.snapshots.stock.enabled` (`OSS_SNAPSHOTS_STOCK_ENABLED`), default off: reading another system's files is opt-in, and the owner decided it is.
- Refused 501, before any write: `include_global_state`, a snapshot that is partial or failed, a shard snapshot whose files were written by a Lucene version this node's reader does not open, and a source-only snapshot (no stored fields to read).

## Consequences

- Easier: a tenant restores a stock snapshot without a Java node, over an operator-named directory or bucket; the native path is untouched.
- Harder: a SMILE decoder and a stock layout reader to maintain against OpenSearch 3.x; a restore's request count scales with files (each file one write and one ranged read for each buffer), like the native restore's.
- Rules out for now: restoring into an existing index, a LIST fallback for a missing `index.latest`, a stock repository the node cannot reach with its own credentials, and reading a repository whose root is not under an allow-listed directory.
