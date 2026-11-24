# Simulate an ingest pipeline

## Serves

The last of the ingest part of the snapshots, templates, ingest and auth milestone: `POST|GET /_ingest/pipeline/_simulate` and `POST|GET /_ingest/pipeline/{id}/_simulate`. A client tries a pipeline on sample documents before an index uses it. Builds on [pipeline-routes](../pipeline-routes/SPEC.md), which answered an underscore id 501, and [ingest-pipelines](../ingest-pipelines/SPEC.md), which runs one.

## Change

`oss_coordinator::pipeline_simulate` reads the body and answers the run:

- The body is a JSON object with `docs`, an array of at most 100 documents, each an object with `_source` (an object) and optionally `_index` and `_id` (default `_index` and `_id`, as OpenSearch fills them). The inline form also has `pipeline`, a definition as `PUT` takes one; the stored form (`/{id}/_simulate`) refuses a `pipeline` in the body, as OpenSearch takes only one of them.
- An inline definition is checked as a stored one is (the supported processors), so a definition the node cannot run is 400 `invalid_pipeline` and runs nothing.
- The answer is `{"docs":[...]}`, one entry per document in order: `{"doc":{"_index":..,"_id":..,"_source":<what the pipeline made>}}`, or for a document the pipeline failed `{"error":{"root_cause":[{"type":"illegal_argument_exception","reason":R}],"type":"illegal_argument_exception","reason":R}}`. A failing document does not fail the rest.
- `?verbose=true` is 501 (per-processor results are not served). A stored id that is missing is 404 `resource_not_found_exception`.

The node serves the four routes under `Config::ingest_pipelines`, beside the pipeline routes. A stored pipeline is read from the snapshot the pipeline routes read (one marker GET when unchanged); an inline one reads nothing. The `_ingest.timestamp` metadata OpenSearch adds to each doc is not added.

## Acceptance criteria

- AC-1: the body is read: `docs` required and non-empty and at most 100, each `_source` an object, `_index` and `_id` defaulted; the inline form requires a valid `pipeline`, the stored form refuses one. Tests: `the_body_is_read_and_its_faults_named`, `an_inline_pipeline_the_node_cannot_run_is_refused`.
- AC-2: the answer runs each document in order, a failing one answers its error alone. Test: `each_document_is_run_and_a_failing_one_does_not_fail_the_rest`.
- AC-3: the node serves the inline and stored routes over `POST` and `GET`, 404 for a missing stored id, 501 for `verbose=true`, and nothing under `_simulate` is stored. Test: `a_pipeline_is_simulated_inline_and_by_id_without_storing_anything` (crates/node/tests/server/pipeline_route.rs).
- AC-4: a simulation by id is one marker GET of an unchanged set and an inline one no request; never a LIST. Test: `a_simulation_reads_the_set_once_and_an_inline_one_not_at_all` (same file).
- AC-5: the routes exist only with the setting on (the existing switch). Test: `the_pipeline_routes_are_served_only_when_pipelines_are_on` extended with `_simulate`.

## S3 request budget

By id: the pipeline routes' get, one marker GET when the set is unchanged. Inline: none. No LIST. No write.

## Bounds

The body is read under the 1 MiB document bound, at most 100 documents, and each run is the pipeline engine's, which does not grow a document beyond what its processors append.

## Switch

None of its own: it rides `Config::ingest_pipelines`, the switch of the pipeline routes, which is off by default.

## Risks

The answer has no `_ingest.timestamp`, and `verbose` is refused; a client that reads either gets a documented difference, not a wrong value.
