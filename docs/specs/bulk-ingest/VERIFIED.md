# Bulk runs ingest pipelines: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-writer --test bulk_ingest --test bulk_parse --test bulk_items`, `cargo clippy -p oss-writer --all-targets -- -D warnings`.

- AC-1: `the_document_a_pipeline_made_is_the_one_applied`
- AC-2: `a_missing_or_failing_pipeline_fails_that_item_alone`
- AC-3: `a_delete_skips_its_pipeline_and_without_ingest_a_pipeline_is_missing`
