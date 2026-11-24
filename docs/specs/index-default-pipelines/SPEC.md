# An index's default and final pipelines

## Serves

The ingest part of the snapshots, templates, ingest and auth milestone: the `index.default_pipeline` and `index.final_pipeline` settings. [bulk-ingest](../bulk-ingest/SPEC.md) ran only the pipeline a request or item names and listed these two as not done; [index-create-settings](../index-create-settings/SPEC.md) lets a create store settings the node reads.

## Change

An index's settings (the descriptor's settings object, flat or nested) may hold two pipeline ids, as OpenSearch's `IngestService` reads them:

- `index.default_pipeline`: runs over a write that names no pipeline. A write that names one (the request's `pipeline` parameter or an item's `pipeline`) runs that instead, and `_none` runs no default.
- `index.final_pipeline`: runs after the first, over what it made, on every write to the index, whatever the write named, `_none` included. A request cannot skip it.

A value of `_none` for either setting runs nothing. Both apply to `index` and `create` items and `_doc` writes, never to a delete, and to the index an alias resolves to, by that index's settings. A pipeline that is missing or fails fails the item as a named one does (400, `pipeline with id [..] does not exist` or the processor's message), and a set that cannot be read fails it 500, so a node with ingest pipelines off fails the item too.

The writer's `IndexInfo` gains `default_pipeline` and `final_pipeline`, read by the node's descriptor routes from the settings it already reads for routing. A create may carry both: the value is a non-empty string, and a node with ingest pipelines off refuses it 501 `unsupported_index_option` (the item would fail on every write).

## Acceptance criteria

- AC-1: with no pipeline named, the default runs; a named pipeline runs instead of it; `_none` runs neither it nor the default. Test: `the_default_runs_unless_a_pipeline_is_named_or_none` (crates/writer/tests/bulk_default_pipelines.rs).
- AC-2: the final runs after the first over what it made, on a write that named a pipeline, named none, or named `_none`; `_none` as the setting runs nothing. Test: `the_final_runs_after_whatever_the_write_named`.
- AC-3: a missing or failing default or final fails that item alone with the messages of AC-4 in bulk-ingest, and a delete runs neither. Test: `a_missing_default_or_final_fails_its_item_and_a_delete_runs_neither`.
- AC-4: the node reads the two settings from the descriptor, flat or nested, none when unset or empty. Test: `the_routes_read_the_default_and_final_pipelines` (crates/node/tests/roles/descriptor_routes.rs).
- AC-5: a create stores them when the node's pipelines are on, and refuses them 501 when off; an empty value is refused 400. Test: `a_booted_index_applies_its_default_and_final_pipelines_and_a_node_without_pipelines_refuses_them` (crates/node/tests/server/boot_templates_ingest.rs).
- AC-6: an end-to-end bulk to an index with a default and a final pipeline applies both, in order. Test: `a_booted_index_applies_its_default_and_final_pipelines_and_a_node_without_pipelines_refuses_them` (crates/node/tests/server/boot_templates_ingest.rs).

## S3 request budget

Unchanged per request: the pipeline set is read once for a request that runs any pipeline (one marker GET, [bulk-ingest](../bulk-ingest/SPEC.md)). An index with a default or final pipeline makes every bulk to it that one read; an index without makes none. The settings come from the descriptor the routes already read. No LIST.

## Switch

None: an index sets neither unless a client asks, and the routes and pipelines are each behind their own switches already.

## Risks

With ingest pipelines on, a `_doc` write now holds the run's two buffers whether or not it names a pipeline, as a bulk does, because the index's own pipelines may run. A node with pipelines off holds none.


A create that sets `default_pipeline` before the pipeline exists succeeds, as in OpenSearch, and writes fail until it does. Whether a pipeline the settings name is deleted is not checked.
