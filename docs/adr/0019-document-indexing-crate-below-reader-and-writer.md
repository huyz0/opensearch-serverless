# 0019: Turning a document into Lucene fields moves to a crate below the reader and the writer

**Status:** Accepted after independent review, 2026-11-18.

## Context

[ADR 0018](0018-tail-overlay-in-the-reader.md) has the reader build one small in-memory segment from the writes a writer pushed. Building it means parsing each source against the index's mapping and producing the Lucene document the writer would have produced: the `mapping`, `document` and `lucene` modules of `oss-writer` (about 6,000 lines). `oss-reader` and `oss-writer` are siblings at layer 3 and never depend on each other ([architecture](../architecture.md)), so the reader cannot call them. Copying them would let the overlay's documents drift from the writer's, and the results would differ from the commit's for the same source.

## Decision

- **A new crate `oss-doc`** takes the `mapping`, `document` and `lucene` modules from `oss-writer`, moved without behaviour change, with their tests and `port-map.tsv` rows. It depends on `oss-rest` (the JSON tokenizer, its only workspace dependency), `chunkjson` and the `lucene-*` crates, so it sits at layer 2, below both; it is added to `LAYERS` in `check-layering.py` and to the architecture table.
- **`oss-writer` re-exports the three modules** (`pub use oss_doc::{document, lucene, mapping}`) so its paths and callers stay as they are; `mapping_growth`, `bulk`, `index`, `publish` and the rest stay in the writer.
- **`oss-reader` depends on `oss-doc`** for the overlay builder of ADR 0018. It still never depends on `oss-writer`.
- **The move is one PR on its own**, behaviour-preserving: the writer's and the conformance tests pass unchanged, with no test edited or removed, and coverage and the mutation gate hold on the moved code.

## Consequences

- One definition of "this source with this mapping as a Lucene document" for the commit and the overlay; the overlay cannot disagree with the writer.
- A new workspace crate: high-risk, per the how-to. It adds no request, no memory use and no setting.
- Rules out: a copy of the modules in the reader, and a dependency between reader and writer.
