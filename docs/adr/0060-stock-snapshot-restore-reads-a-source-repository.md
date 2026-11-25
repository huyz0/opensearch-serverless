# 0060: A stock snapshot is restored by reading its repository through a source store

**Status:** Proposed

## Context

A tenant moves data in from a stock OpenSearch cluster by restoring a snapshot that cluster wrote to an `fs` or `s3` repository (the `BlobStoreRepository` layout of OpenSearch 3.x). The native repository ([ADR 0042](0042-snapshots-taken-shallow-first.md), [ADR 0045](0045-shallow-snapshot-restore.md)) is different in kind: its snapshot lives in this deployment's object store and records which published blobs hold each shard, copying nothing. A stock repository holds copied Lucene files, chunked, under its own root, with metadata written in SMILE, optionally compressed, behind a Lucene checksum header and footer.

Stock writes `index.latest` for every repository type (`writeIndexGen` does it unconditionally); it falls back to a LIST of `index-` only when that blob is missing. A LIST is priced like a PUT here.

The owner decided stock interop is wanted, opt-in, restore first (the parity plan's R14.8).

## Decision

**Which repository.** A repository registered with the setting `format: "stock"` (a key only this node knows) is read as a stock repository; without it a repository is native, as today, and the native rows of the request-budget table stay as they are. Registering a repository is the admin action the [authorization table](0051-authorization-at-the-route-by-action-table.md) already gates; registering a stock one needs no weaker a role.

**The source store.** The repository reads through a **source store**, read-only by type: a wrapper over `oss-blob` that offers reads and answers every write and delete with an error (a test calls each). Metered like every store.

- `fs`: a directory under an operator allow-list of roots, `OSS_STOCK_REPO_ROOTS`, empty by default, so nothing is readable until one is named. `location` is canonicalised and must stay under a root; a symlink anywhere in the path is refused outright (no check-then-use), and files are opened relative to the opened root.
- `s3`: a bucket and `base_path` under an allow-list of `bucket/prefix` roots, `OSS_STOCK_REPO_S3_ROOTS`, empty by default, under the node's own credentials and endpoint. A source that equals or overlaps this deployment's own bucket and prefix is refused whatever the list says, since that bucket holds other tenants' data.

**Restore only.** Export to a stock repository is not decided here and needs its own ADR.

**No LIST, and a stated depth.** The generation comes from `index.latest`; a repository without it is refused 400. The reads form four rounds before any data byte: (1) `index.latest`; (2) `index-N`; (3) the snapshot's `snap-<uuid>.dat` and one `meta-<uuid>.dat` for each *requested* index, in parallel; (4) one shard `snap-<uuid>.dat` for each shard of each requested index, in parallel (fan-out bounded at 8). The shard file lists are read before the first write, so every refusal that needs them (a Lucene version the reader does not open, a source-only snapshot, too many files) comes before anything is written, at the price of that wider fourth round. Data blobs then stream. The metadata of indices the request did not name is never read.

**Bounds, as numbers.** An `index-N` is read whole into one buffer reserved from the request-body pool, at most 64 MiB; a metadata or shard blob at most 16 MiB, and 64 MiB after decompression; a restore of more than 100 indices or 4,096 shards is 400 `snapshot_too_large`, as the native restore; a shard of more than 10,000 files, or a file of more than 10,000 chunks, is refused 501. The SMILE decoder allows depth 64, 1,024 shared key and 1,024 shared string entries (the format's own maximum), a string of at most 16 MiB and 64 MiB of output, reserving before it allocates; a back-reference past its table, a truncated length or a document over a bound is refused naming the blob.

**Metadata.** The index's mapping and settings come from SMILE (`meta-<uuid>.dat`), decoded to JSON by a sans-IO decoder in `oss-format` (a foreign wire format, next to the other blob formats), then through the same mapping and settings parsing a create uses, so a mapping the Rust node cannot index (a `knn_vector` it does not serve, an unsupported type) is refused as a create refuses it. The native restore's name resolution (`rename_pattern`, collisions, existing and system indices) and its `restorableSettings` stripping are reused as they are: a stock index's own uuid, creation date, version, provided name and replica count are dropped, the shard count is kept, and a stock index with a routing partition size above 1 or custom routing is refused 501.

**Files.** Each shard's files are copied as whole-file blobs into the new index's first-term directory by the native restore's streaming copy, a `__<uuid>.partN` chunk set joined in order (the copy buffer is independent of the chunk size). The Lucene footer's CRC is computed over the bytes while they stream and must equal the footer's own checksum and the one the stock file list records, and the length must equal the recorded length; a mismatch, or a missing chunk, fails the restore. A failed restore is cleaned as the native one is: the descriptor is created first, marked, so deleting each index created purges every blob written under it, including a half-written file. The manifest is built from the stock file list and lands as the native restore's does, under the same mark and swap ([snapshot-restore](../specs/snapshot-restore/SPEC.md) AC-9, inherited, not re-specified).

**Switch.** `serverless.snapshots.stock.enabled` (`OSS_SNAPSHOTS_STOCK_ENABLED`), default off. Both sides are tested: off answers 501 with no request; on restores.

**Docs.** The setting is added as [how-to/add-a-setting](../how-to/README.md) says, `docs/compatibility.md` gets the stock restore and its refusals, and `docs/request-budgets.md` the rows the spec names.

## Consequences

- Easier: a tenant restores a stock snapshot without a Java node, from an operator-named directory or bucket; the native path is untouched.
- Harder: a SMILE decoder and a stock layout reader to maintain against OpenSearch 3.x; a restore costs one write and some ranged reads for each file, like the native restore's, plus the four metadata rounds.
- Rules out for now: restoring into an existing index, a LIST fallback for a missing `index.latest`, a source not under an allow-listed root, and export to stock.
