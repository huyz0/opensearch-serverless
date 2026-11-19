# oss-coordinator

The coordinating role. REST, request parsing, routing, fan-out and reduce.

## Owns
The REST surface, shard routing, fan-out to readers and writers, result merging.

## Depends on
`oss-types`, `oss-blob`, `oss-cluster`, `oss-format`, `oss-reader`, `oss-writer`.

## Entry points
`src/lib.rs`. Built third; see [docs/roadmap.md](../../docs/roadmap.md).
