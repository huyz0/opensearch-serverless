# oss-reader

The search role. Serves queries from immutable published segment blobs plus the tail records a writer pushed to it. Never writes to the object store.

## Owns
Opening published commits, the page cache over ranged segment reads, tail-record search, and the shard-level query and fetch phases: the search DSL subset (`dsl/`), its translation to lucene-rust queries (`lucene_query`), the search (`search`), the metric aggregations and `top_hits` (`aggs/metric`) and the streamed response (`response`).

## Depends on
`oss-types`, `oss-blob`, `oss-format`, `oss-doc` (a source as a Lucene document, for the overlay), `oss-rest` (its JSON tokenizer and writer), and lucene-rust (`lucene-store`, `lucene-index`, `lucene-search`, `lucene-codecs`, `lucene-analysis`, `lucene-aggs`, pinned in the root `Cargo.toml`). Never `oss-writer` (siblings stay independent).

## Entry points
`open_shard(store, cache, base, index, shard)` opens a shard's latest published commit, or in two steps `ShardFiles::open_with_meta` then `open_located`, with `open_cost::open_bytes` between them estimating what the open keeps in the heap ([ADR 0014](../../docs/adr/0014-open-indexes-pool.md)); `ShardFiles::open` finds its files with two GETs; `EngineStore` is lucene-rust's `BlobStore` over a commit. Contract: [docs/specs/reader-open-commit/SPEC.md](../../docs/specs/reader-open-commit/SPEC.md).

`dsl::source::tokenizer` and `dsl::Recorder` read a body a chunk at a time; `SearchSource::parse` gives the request; `search::search` runs it over an `OpenShard`; `response::write_response` writes it as hits are fetched (the hits of every `top_hits` announced with them, one fetch round), then the aggregations; `aggs::metric::partial::reduce` reduces their shard results. Contract: [docs/specs/reader-search/SPEC.md](../../docs/specs/reader-search/SPEC.md).

`overlay::Overlay::build` makes one in-memory segment from the writes a writer pushed above a commit's checkpoint (latest version of each id, the ids it replaces or deletes), and `overlay::estimate_bytes` bounds its heap for the pool charge ([ADR 0018](../../docs/adr/0018-tail-overlay-in-the-reader.md)). Contract: [docs/specs/reader-overlay-segment/SPEC.md](../../docs/specs/reader-overlay-segment/SPEC.md).

`overlay::find_copies` looks up the commit's live copies of the ids an overlay replaces, `overlay::candidate_ids` names them before the build, `Overlay::build_over` drops records the commit already holds and `Overlay::hidden` lists the commit documents to hide (contract: [docs/specs/reader-overlay-commit/SPEC.md](../../docs/specs/reader-overlay-commit/SPEC.md)).

`overlay::Masks` and `overlay::combine` give the searcher one segment list of the commit (hidden documents masked) and the overlay, and `Overlay::norms` its norms (contract: [docs/specs/reader-overlay-view/SPEC.md](../../docs/specs/reader-overlay-view/SPEC.md)).

`OpenShard::with_overlay` (through `Overlay::view_of`) is the shard a search sees with the overlay on top of its commit: the same shared directory, a reader over the masked commit segments and the overlay's (contract: [docs/specs/reader-overlay-search/SPEC.md](../../docs/specs/reader-overlay-search/SPEC.md)).

`overlay::Ready` (a finished overlay with the commit's copies of its ids) and the `overlay::OverlaySource` trait are what a search asks for the overlay of the shard it serves: finished, or nothing (contract: [docs/specs/reader-overlay-source/SPEC.md](../../docs/specs/reader-overlay-source/SPEC.md)).
