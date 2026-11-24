# Ingest pipeline REST routes

## Serves

The ingest part of the snapshots, templates, ingest and auth milestone: `PUT|POST|GET|DELETE /_ingest/pipeline/{id}` and `GET /_ingest/pipeline` over the ingest pipeline document set ([doc-sets](../doc-sets/SPEC.md)). Running a stored pipeline on a write is a separate piece.

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `rest/PipelineHandler.java` (its put, get and delete routes). The Java validates a pipeline by compiling it with its processor engine; this build has none yet, so `oss_coordinator::pipeline_admin::validate` accepts a pipeline only when it has `processors` and every processor it names, under `on_failure` too (nested at most 8 deep), is one of `SUPPORTED` (`append`, `convert`, `lowercase`, `remove`, `rename`, `set`, `trim`, `uppercase`). Any other is 400 `invalid_pipeline` with the available list, as the Java words it, so a pipeline is never stored that the node cannot run.

- A put or post replaces the pipeline; a delete removes one id (404 `pipeline_missing` when absent).
- A get selects all (no id, `*`, `_all`) or one id; several ids and other wildcards are 501, as in the Java.
- An underscore-prefixed id (`_simulate`) is 501 for a put, post, delete or get of a pipeline; `_simulate` as a path segment is served by [pipeline-simulate](../pipeline-simulate/SPEC.md).
- The routes are registered when `Config::ingest_pipelines` is on.

## Acceptance criteria

- AC-1: A pipeline of supported processors is accepted; an unsupported one, a malformed body, a missing `processors` and over-deep `on_failure` are refused. Tests: `a_pipeline_of_supported_processors_is_accepted`, `an_unsupported_processor_is_refused_with_the_available_list`, `a_malformed_pipeline_is_refused`, `on_failure_nesting_is_bounded`.
- AC-2: A mutation names one concrete id; a get selects one or all. Tests: `a_mutation_names_one_concrete_id`, `an_id_selects_one_or_all`, `a_get_answers_an_object_of_ids`.
- AC-3: A pipeline is stored, read, listed, replaced and removed over REST. Test: `a_pipeline_is_stored_read_listed_and_removed`.
- AC-4: What cannot be run is refused and stores nothing. Test: `what_the_node_cannot_run_is_refused_and_stores_nothing`.
- AC-5: A read of an unchanged set is one GET and never a LIST. Test: `a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed`.
- AC-6: The routes exist only with the setting on. Test: `the_pipeline_routes_are_served_only_when_pipelines_are_on`.

## S3 request budget

As [template-routes](../template-routes/SPEC.md): a put is the doc-set write budget, a get of an unchanged set one marker GET. No LIST. Off: no route, no request.

## Switch

`Config::ingest_pipelines`, default off: a stored pipeline is not yet run by a write, so turning it on before that piece lands would store configuration that does nothing.

## Risks

The allowlist is the node's whole processor surface until the engine piece; a pipeline written for a richer engine is refused, not stored.
