# Ingest pipeline engine: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-coordinator --test ingest --test pipeline_admin`, `cargo clippy -p oss-coordinator --all-targets -- -D warnings`.

The expectations come from reading the ingest-common processors, not a recorded run.

- AC-1: `set_writes_a_field_and_honours_override_and_ignore_empty_value`
- AC-2: `remove_and_rename_move_fields_and_report_missing_ones`
- AC-3: `text_processors_change_case_and_trim_into_a_target`
- AC-4: `append_builds_a_list_and_can_skip_duplicates`
- AC-5: `convert_changes_types_and_refuses_what_does_not_parse`
- AC-6: `failures_are_taken_by_on_failure_or_ignore_failure`, `processors_run_in_order`
- AC-7: `what_the_engine_does_not_run_is_refused_at_compile`, `a_malformed_pipeline_is_refused`, `an_unsupported_processor_is_refused_with_the_available_list`, `a_pipeline_of_supported_processors_is_accepted`
