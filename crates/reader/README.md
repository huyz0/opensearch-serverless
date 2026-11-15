# oss-reader

The search role. Serves queries from immutable published segment blobs plus the tail records a writer pushed to it. Never writes to the object store.

## Owns
Opening published commits, the page cache over ranged segment reads, tail-record search, and the shard-level query phase.

## Depends on
`oss-types`, `oss-blob`, `oss-format`, and lucene-rust (`lucene-store`, `lucene-index`, `lucene-search`, pinned in the root `Cargo.toml`). Never `oss-writer` (siblings stay independent).

## Entry points
`open_shard(store, cache, base, index, shard)` opens a shard's latest published commit; `ShardFiles::open` finds its files with two GETs; `EngineStore` is lucene-rust's `BlobStore` over a commit. Contract: [docs/specs/reader-open-commit/SPEC.md](../../docs/specs/reader-open-commit/SPEC.md).
