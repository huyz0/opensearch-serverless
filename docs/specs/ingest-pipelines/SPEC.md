# Ingest pipeline engine

## Serves

The ingest part of the snapshots, templates, ingest and auth milestone: running a stored pipeline over a document. Replaces the processor allowlist of [pipeline-routes](../pipeline-routes/SPEC.md) with one compile step shared by the routes and the write path. The bulk and document routes calling it follow in their own changes.

## Change

`oss_coordinator::ingest::Pipeline::compile(definition)` and `Pipeline::run(source)`, over the JSON tree of `oss_doc`. No store call.

Processors, with the options each takes: `set` (`field`, `value`, `override`, `ignore_empty_value`), `remove` (`field` as one name or a list, `ignore_missing`), `rename` (`field`, `target_field`, `ignore_missing`), `lowercase`, `uppercase`, `trim` (`field`, `target_field`, `ignore_missing`), `append` (`field`, `value`, `allow_duplicates`), `convert` (`field`, `target_field`, `ignore_missing`, `type` of `integer`, `long`, `float`, `double`, `boolean`, `string`). Every processor also takes `tag`, `description`, `ignore_failure` and `on_failure`; a pipeline takes `description`, `version`, `_meta` and `on_failure`. Fields are dotted paths into nested objects.

Refused at compile, so a stored pipeline never silently ignores part of itself: another processor type (with the available list), another option (`if`, `copy_from`, and so on), a `{{` template in a value, a missing required option, a non-boolean flag, `on_failure` nested over 8 deep.

A failing processor runs its own `on_failure`, else is skipped under `ignore_failure`, else fails the document (the pipeline's `on_failure` runs first when there is one). Failure messages follow the ingest-common ones where the Java text is known.

`pipeline_admin::validate` now answers what `Pipeline::compile` answers.

## Acceptance criteria

- AC-1: `set` writes nested fields and honours `override` and `ignore_empty_value`. Test: `set_writes_a_field_and_honours_override_and_ignore_empty_value`.
- AC-2: `remove` and `rename` move fields and report missing ones. Test: `remove_and_rename_move_fields_and_report_missing_ones`.
- AC-3: Text processors change case and trim into a target. Test: `text_processors_change_case_and_trim_into_a_target`.
- AC-4: `append` builds a list and can skip duplicates. Test: `append_builds_a_list_and_can_skip_duplicates`.
- AC-5: `convert` changes types and refuses what does not parse. Test: `convert_changes_types_and_refuses_what_does_not_parse`.
- AC-6: Failures are taken by `on_failure` or `ignore_failure`; processors run in order. Tests: `failures_are_taken_by_on_failure_or_ignore_failure`, `processors_run_in_order`.
- AC-7: What the engine does not run is refused at compile, and the routes refuse the same. Tests: `what_the_engine_does_not_run_is_refused_at_compile`, `a_malformed_pipeline_is_refused`, `an_unsupported_processor_is_refused_with_the_available_list`, `a_pipeline_of_supported_processors_is_accepted`.

## The `script` processor

With `serverless.scripting.painless.native_compile` on, a `script` processor (`source`, `lang` of `painless` or absent, `params` an object) runs the Painless source over the document as `ctx`, through the Rust front end and runtime, and keeps the document the script leaves (maps and lists edited in place, fields added or removed). A stored script (`id`), another `lang`, or any other option is refused at compile; with the switch off the type is refused as an unsupported processor. A script that does not compile, that the runtime does not run exactly as Painless does, or that throws fails the document with a `script_exception` message, and `on_failure` or `ignore_failure` take it as for any processor. The source is compiled for each document.

- AC-8: The `script` processor edits a document behind the switch and is refused without it, and its failures are taken like any processor's. Test: `the_script_processor_is_refused_until_the_switch_is_on_then_edits_the_document` (own test binary `ingest_script`).

## S3 request budget

None: no store call.

## Switch

None needed: nothing on the write path calls it yet; the routes stay behind `Config::ingest_pipelines`.

## Risks

The expectations come from reading the ingest-common processors, not a recorded run of core; float and double conversion print as `Double.toString` does. `convert` to `float` parses as a double.
