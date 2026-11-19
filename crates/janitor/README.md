# oss-janitor

The cleanup role: reclaims what nothing names any more, without a lock and without a LIST on a request path.

## Owns
The garbage collector for segment blobs (`gc`): one shard's sweep, judging each blob in a term older than the published manifest's by whether the manifest or a pin names it, with the two-sweep grace carried in the caller's memory and a dry-run switch. Later passes: own-log and dead-node log cleanup, published sweeps, tombstone and reclaim scheduling.

## Depends on
`oss-blob` (the store and its meter), `oss-format` (the manifest and the shard's storage path), `oss-types` (the dry-run setting). Never `oss-reader` or `oss-writer`.

## Entry points
`gc::Collector::sweep`, one shard, over any `BlobStore`, with a `gc::Pins` the caller supplies and a `gc::Memory` it keeps between sweeps; `settings::DRY_RUN`. Tested on the emulator with exact request counts (`tests/`). Spec: [docs/specs/janitor-gc](../../docs/specs/janitor-gc/SPEC.md); decision: [ADR 0020](../../docs/adr/0020-janitor-crate.md).
