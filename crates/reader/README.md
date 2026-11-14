# oss-reader

The search role. Serves queries from immutable published segment blobs plus the tail records a writer pushed to it. Never writes to the object store.

## Owns
Opening published commits, the page cache over ranged segment reads, tail-record search, and the shard-level query phase.

## Depends on
`oss-types`, `oss-blob`. Never `oss-writer` (siblings stay independent).

## Entry points
`src/lib.rs`. Built first; see [docs/roadmap.md](../../docs/roadmap.md).
