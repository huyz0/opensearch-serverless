# oss-janitor

The cleanup role: reclaims what nothing names any more, without a lock and without a LIST on a request path.

## Owns
The garbage collector for segment blobs (`gc`): one shard's sweep, judging each blob in a term older than the published manifest's by whether the manifest or a pin names it, with the two-sweep grace carried in the caller's memory and a dry-run switch. The dead-node log collector (`node_log`): deletes a dead node's write log once every shard it names has published past it, reading the log and commits through two seams the node implements. The published sweeps (`published::Sweeps`): which shards a pass sweeps, the ones that dropped a file, are watched, were forced or are new. Later passes: tombstone and reclaim scheduling.

## Depends on
`oss-blob` (the store and its meter), `oss-format` (the manifest, the shard's storage path, leases and log keys), `oss-cluster` (leases, shard heads), `oss-meta` (descriptors), `oss-types` (the settings). Never `oss-reader` or `oss-writer`.

## Entry points
`gc::Collector::sweep`, one shard, over any `BlobStore`, with a `gc::Pins` the caller supplies and a `gc::Memory` it keeps between sweeps; `node_log::Collector::collect` with `node_log::{NodeLogs, FencedLog, Commits}`; `published::Sweeps::sweep_published`; `settings::DRY_RUN`, `settings::MINIMUM_UNREFERENCED_MILLIS`. Specs: [docs/specs/janitor-published](../../docs/specs/janitor-published/SPEC.md), [docs/specs/janitor-node-log](../../docs/specs/janitor-node-log/SPEC.md). Tested on the emulator with exact request counts (`tests/`). Spec: [docs/specs/janitor-gc](../../docs/specs/janitor-gc/SPEC.md); decision: [ADR 0020](../../docs/adr/0020-janitor-crate.md).
