# 0044: Ingest pipelines run in the write path through a per-request hook, behind a switch

**Status:** Proposed

## Context

Pipeline CRUD stores pipelines as a document set ([ADR 0031](0031-document-sets.md)). Running one over a document belongs on the write path, which is durability territory: the document an item applies is no longer the one the client sent. The writer cannot depend on the coordinator that holds the engine ([ADR 0001](0001-strangler-and-crate-layout.md)). A pipeline read an item would add requests that scale with documents.

## Decision

- The writer owns a small `Ingest` trait (`run(pipeline id, source) -> document`). `Bulk::with_ingest` takes one for the request. An item naming a pipeline other than `_none` has the pipeline run after its index checks, and the result replaces the source before it is applied. Delete never runs a pipeline.
- The node implements the trait over the pipeline set: one `Run` a request that reads the set through the node's snapshot cache the first time a pipeline is named (one marker GET when unchanged; when the set moved, one more GET for each pipeline whose content changed, at most 1,000, so the cost follows the pipelines and not the documents), and compiles each named pipeline once. An item adds no request. A request that names none reads nothing. No LIST.
- A missing pipeline or a failing processor fails that item with 400, an unreadable set with 500; other items are unaffected. Without an `Ingest` a named pipeline fails as before.
- The engine runs only what it can run exactly: eight processors, dotted field paths, no templates or conditions. Everything else is refused when the pipeline is stored, so a stored pipeline never silently ignores part of itself.
- Memory: the run's two read buffers are reserved from the request-body pool. The document a pipeline returns is one allocation an item, sized by its source plus what the stored pipeline adds (each `set` or `append` value is inside a pipeline of at most 1 MiB); a slice holds the rewritten items beside the originals until it is applied, within the slice's existing bulk-budget grant. That output is not reserved separately: a known gap, bounded per item and by the slice.
- Behind `Config::ingest_pipelines`, default off. A request reserves the run's two buffers from the request-body pool before it reads.

## Consequences

- Easier: pipelines work on `_bulk`, `_doc` and `_create` at one marker read a request, whatever the document count.
- Harder: a write's stored document can differ from its source; the node log records the source after the pipeline, so replay needs no pipeline.
- Trades away some OpenSearch compatibility: a pipeline using another processor, a template or a condition is refused when stored (with the list of supported processors), and the settings above are not honoured. This is stated at the route, never silent, and the owner decides whether it stands.
- Rules out: processors that call out, for now. The `default_pipeline` and `final_pipeline` settings (spec `index-default-pipelines`) and `_simulate` (spec `pipeline-simulate`) were first ruled out and were then added without changing this decision: the engine and where it runs are as above, and the settings only choose which stored pipeline runs.

## Widening the allowlist

A processor is added in three places: its name in `PROCESSORS` (`crates/coordinator/src/ingest/mod.rs`, which also builds the 400 text listing what is supported), its options in `ingest/compile.rs` (every option it does not run is refused there), and its step in `ingest/run.rs`. Each new processor needs a test of its own in `crates/coordinator/tests/ingest.rs` and a line in `docs/specs/ingest-pipelines/SPEC.md`; a processor that reads outside the document (a lookup, an HTTP call) does not belong in this engine.
