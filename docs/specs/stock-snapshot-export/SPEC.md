# Export to a stock OpenSearch snapshot repository

## Serves

Parity plan R14.8, [ADR 0062](../../adr/0062-a-snapshot-is-exported-to-a-stock-repository-by-copying.md): `PUT /_snapshot/{repo}/{name}` where `{repo}` is registered with `format: "stock"`, not `readonly`, over an `fs` or `s3` target takes a copying snapshot of the named indices that a stock OpenSearch 3.10 node restores. The restore half is [stock-snapshot-restore](../stock-snapshot-restore/SPEC.md).

## Acceptance criteria

- AC-1: A SMILE encoder in `oss-format` writes the JSON subset the stock blobs use (objects, arrays, strings, integers, longs, booleans, null, doubles, shared keys, binary), with the decoder's bounds (depth 64, 1,024 shared keys, 64 MiB of output). Every document it writes decodes, with the existing decoder, to the JSON it was given, and Jackson's own Smile reader (a Java check over the same bytes, `fixtures/snapshots/stock-smile/`) reads it to the same tree. Tests: round trip at every value type, at each bound and one past it; the Java check by hand.
- AC-2: The Lucene header and footer writer produces a blob the envelope reader accepts (codec name, version 1, SMILE body, footer CRC), and `CodecUtil.checksumEntireFile` and `retrieveChecksum` accept (Java check over the same bytes). Tests: the reader over writer output at every body length, and the Java check.
- AC-3: The `RepositoryData` writer produces an `index-N` the existing reader parses to the same snapshots and indices, and that the server's own `RepositoryData.snapshotsFromXContent` parses (Java check); the next generation extends the previous with the new snapshot, keeps every earlier snapshot and index id, and a repository with neither `index.latest` nor `index-N` starts at generation 0. Tests: extend the recorded repository's `index-0` and read it back; the Java check.
- AC-4: The shard snapshot, snapshot info, index metadata and empty global metadata writers produce blobs the existing readers decode to the source index's mapping, settings (with `index.number_of_shards`, `index.number_of_replicas` 0 and a uuid and creation date, which stock requires) and files. Tests: each writer over the recorded index decodes equal to the recorded blob's decoding; the Java check parses each with the server's own `*_FORMAT`.
- AC-5: The take copies each file of a shard's published commit as a whole-file blob, streaming through two buffers, with the Lucene footer CRC and length checked as the bytes pass; a file whose bytes do not match its manifest length or checksum fails the take before any blob names it in `index-N`. The commit is pinned with the native protective marker for the copy and released after. Tests: a taken index's repository holds each file's bytes exactly; a planted truncated and a planted flipped file fail the take; the marker is present during the copy and gone after (a store that records the order of calls).
- AC-6: Order and fencing: shard files, shard snapshots, index metadata, snapshot info and global metadata are written before `index-N+1`; `index-N+1` is a conditional create and `index.latest` the last put. Two takes into one repository at once: one succeeds and the other fails with 409, leaving orphan blobs and a repository that restores. A failure at every store call leaves a repository whose `index.latest` still names a restorable generation. Tests: a store that records the order; two concurrent takes; a fault at each call.
- AC-7: Refused 501 before any write: an index in a bucket or shared packs, `include_global_state`, `partial`, a file above the store's single-object limit, a delete of a stock snapshot, a mapping or settings stock cannot restore; 400 for a name over 128 bytes or outside `[A-Za-z0-9._-]`; 404 for an unknown index; a repository registered `readonly` is 400. Tests: one request each, each asserting no write reached the target.
- AC-8: A target outside the export allow-lists (`OSS_STOCK_EXPORT_FS_ROOTS`, `OSS_STOCK_EXPORT_S3_ROOTS`, empty by default) is refused 400 and nothing is written; an `fs` location reached by `..` or a symlink, and an `s3` target overlapping this deployment's own bucket and prefix, are refused. The target store answers every delete with an error (a test calls each method) and writes only under the repository prefix. Registering the repository as anyone but a cluster admin is 403. Tests as the restore's AC-7.
- AC-9: With `serverless.snapshots.stock.export.enabled` off (the default) the take answers 501 and makes no request to either store; with it on, the take passes. The native take's rows are unchanged. Tests: both modes.
- AC-10: Requests: per shard one manifest read, one ranged read for each buffer of each file, one put for each file, one put for its snapshot blob and one for its generation; per index one put; per snapshot four puts and the two generation reads; no LIST on `fs` and on the RustFS leg alike; requests scale with files and bytes and not with indices that are idle. Budget rows `snapshot.stock_export.requests` (one index, one shard, the recorded fixture's two files) and `snapshot.stock_export_cleanup.requests` (a failed take), exact, recorded in `docs/request-budgets.md` when measured. A 429 when the pool cannot hold the buffers, writing nothing.
- AC-11: The repository the Rust node wrote restores on a stock 3.10 node and the restored index answers what the source answered. By hand: a stock node (`opensearch-min`) registers the exported directory read-only, restores, and returns the three searches of `fixtures/snapshots/stock/README.md`. Recorded in `VERIFIED.md` with the build and result; NOT-RUN until run.

## Test plan

AC-1 to AC-4: unit and round-trip tests in the format crate, plus a Java check beside `fixtures/snapshots/stock-smile/` for Jackson and the server's `*_FORMAT` classes. AC-5 to AC-10: server tests over an emulator store and an `fs` store, the recorded repository as the template, with a RustFS leg for AC-6 and AC-10 over `s3`. AC-11: the stock node, by hand.

## S3 request budget

See AC-10.

## Bounds

The restore's numbers (ADR 0062): 100 indices, 4,096 shards, 10,000 files a shard, 100,000 file entries, 128-byte names, 16 MiB blobs; two copy buffers reserved before the first read.

## Switch

`serverless.snapshots.stock.export.enabled`, default off: it copies the tenant's bytes out of the deployment.

## Risks

A stock layout detail the recorded repository does not show (found only by AC-11, which is why it is a gate); every snapshot copies all bytes, so a nightly export costs its whole size in requests and bytes; a crash leaves orphan blobs and a pin marker for the native recovery.

## Build order

1. SMILE encoder (AC-1). 2. Header and footer writer, `RepositoryData` writer (AC-2, AC-3). 3. The metadata and shard snapshot writers (AC-4). 4. The copy and the target store (AC-5, AC-8). 5. The take, fencing, route and switch (AC-6, AC-7, AC-9, AC-10). 6. The stock-node proof (AC-11). Each step updates `docs/compatibility.md` and the budget rows it spends.
