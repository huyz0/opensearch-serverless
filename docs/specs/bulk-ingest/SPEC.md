# Bulk runs ingest pipelines

## Serves

The ingest part of the snapshots, templates, ingest and auth milestone: an item of a `_bulk` that names a pipeline has the pipeline run over its source before it is applied. Builds on [ingest-pipelines](../ingest-pipelines/SPEC.md).

## Change

`oss_writer::bulk::Ingest` is a trait with `run(pipeline id, source) -> Result<Vec<u8>, IngestError>`, and `Bulk::with_ingest` gives a request one. The writer cannot depend on the coordinator, so the engine plugs in through the trait; an implementation is made for the request and may keep what it read until the request ends, which is how a bulk costs one read of the pipelines and not one an item.

For an `index` or `create` item that names a pipeline other than `_none`, after its index and alias checks, the document the pipeline made replaces the source the target applies. `IngestError::Missing` fails the item with 400 `illegal_argument_exception` `pipeline with id [..] does not exist`; `IngestError::Failed(reason)` fails it with 400 `illegal_argument_exception` and the processor's message. Other items of the request are unaffected. A `delete` never runs a pipeline. Without an `Ingest`, a named pipeline fails as before.

Not done here: an index's `default_pipeline` and `final_pipeline` settings, and the node implementation (a follow-up change).

## Acceptance criteria

- AC-1: The document a pipeline made is the one applied; `_none` runs nothing. Test: `the_document_a_pipeline_made_is_the_one_applied`.
- AC-2: A missing or failing pipeline fails that item alone with 400. Test: `a_missing_or_failing_pipeline_fails_that_item_alone`.
- AC-3: A delete skips its pipeline, and with no ingest a named pipeline is missing. Test: `a_delete_skips_its_pipeline_and_without_ingest_a_pipeline_is_missing`.

## S3 request budget

None here: the trait makes no store call. The node's implementation reads the pipeline set once a bulk that names a pipeline (one marker GET, plus one GET per moved document).

## Switch

None needed: with no `Ingest` given, behaviour is unchanged. The node gives one only under `Config::ingest_pipelines`.

## Risks

Rewritten items are cloned for the slice, so a slice that names pipelines holds the cloned sources beside the originals, within the slice's existing budget grant.
