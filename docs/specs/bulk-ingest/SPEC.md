# Bulk runs ingest pipelines

## Serves

The ingest part of the snapshots, templates, ingest and auth milestone: an item of a `_bulk` that names a pipeline has the pipeline run over its source before it is applied. Builds on [ingest-pipelines](../ingest-pipelines/SPEC.md).

## Change

`oss_writer::bulk::Ingest` is a trait with `run(pipeline id, source) -> Result<Vec<u8>, IngestError>`, and `Bulk::with_ingest` gives a request one. The writer cannot depend on the coordinator, so the engine plugs in through the trait; an implementation is made for the request and may keep what it read until the request ends, which is how a bulk costs one read of the pipelines and not one an item.

For an `index` or `create` item that names a pipeline other than `_none`, after its index and alias checks, the document the pipeline made replaces the source the target applies. `IngestError::Missing` fails the item with 400 `illegal_argument_exception` `pipeline with id [..] does not exist`; `IngestError::Failed(reason)` fails it with 400 `illegal_argument_exception` and the processor's message. Other items of the request are unaffected. A `delete` never runs a pipeline. Without an `Ingest`, a named pipeline fails as before.

`oss_node::ingest_run::IngestPipelines` is the node's implementation over the ingest pipeline set. A request makes one `Run`: the first pipeline it names reads the set through the node's `DocSetCache` (one marker GET when unchanged), and later items reuse that snapshot and the pipelines compiled from it. `BulkService` and `DocService` take it with `with_ingest`; `assemble` gives it only under `Config::ingest_pipelines`, and the request reserves the run's two buffers from the request-body pool first. A set that cannot be read fails the item with 500 `illegal_state_exception` (`IngestError::Unavailable`).

Not done here: an index's `default_pipeline` and `final_pipeline` settings, and `_simulate`.

## Acceptance criteria

- AC-1: The document a pipeline made is the one applied; `_none` runs nothing. Test: `the_document_a_pipeline_made_is_the_one_applied`.
- AC-2: A missing or failing pipeline fails that item alone with 400. Test: `a_missing_or_failing_pipeline_fails_that_item_alone`.
- AC-3: A delete skips its pipeline, and with no ingest a named pipeline is missing. Test: `a_delete_skips_its_pipeline_and_without_ingest_a_pipeline_is_missing`.

- AC-4: A run reads the set once for the request and tells a missing pipeline from a failing one. Tests: `a_run_keeps_the_set_it_read_for_the_request`, `a_missing_pipeline_and_a_failing_one_are_told_apart`.
- AC-5: A node with pipelines on runs the pipeline a bulk item or a `_doc` write names, and one with them off refuses it. Test: `a_write_runs_the_pipeline_it_names_only_when_pipelines_are_on`, `a_write_that_may_name_a_pipeline_reserves_the_run_s_buffers`.

## S3 request budget

None here: the trait makes no store call. The node reads the pipeline set once for a request that names a pipeline: one marker GET, plus one GET per moved document, by `DocSetCache`; an item adds none. A request that names none reads nothing. Not metered by a test here.

## Switch

None needed: with no `Ingest` given, behaviour is unchanged. The node gives one only under `Config::ingest_pipelines`.

## Risks

A request that may name a pipeline reserves two buffers of `pit_buffer_bytes` from the request-body pool whether or not it does, because a bulk line can name one after the body has started to stream. Rewritten items are cloned for the slice, so a slice that names pipelines holds the cloned sources beside the originals, within the slice's existing budget grant.
