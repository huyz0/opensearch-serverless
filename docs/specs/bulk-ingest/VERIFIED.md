# Bulk runs ingest pipelines: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-writer --test bulk_ingest --test bulk_parse --test bulk_items`, `cargo test -p oss-node --all-features --test server ingest_run a_write_runs`, `cargo clippy -p oss-writer --all-targets -- -D warnings`.

- AC-1: `the_document_a_pipeline_made_is_the_one_applied`
- AC-2: `a_missing_or_failing_pipeline_fails_that_item_alone`
- AC-3: `a_delete_skips_its_pipeline_and_without_ingest_a_pipeline_is_missing`
- AC-4: `a_run_keeps_the_set_it_read_for_the_request`, `a_missing_pipeline_and_a_failing_one_are_told_apart`, `a_stored_pipeline_that_does_not_compile_fails_the_item`
- AC-5: `a_write_runs_the_pipeline_it_names_only_when_pipelines_are_on`, `a_write_that_may_name_a_pipeline_reserves_the_run_s_buffers`
