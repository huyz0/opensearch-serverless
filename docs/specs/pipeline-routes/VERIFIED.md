# Ingest pipeline REST routes: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-coordinator --test pipeline_admin`, `cargo test -p oss-node --all-features --test server pipeline_route assemble`, `cargo clippy --all-targets` for both crates with `-D warnings`.

The expectations come from reading the Java source, not a recorded run.

- AC-1: `a_pipeline_of_supported_processors_is_accepted`, `an_unsupported_processor_is_refused_with_the_available_list`, `a_malformed_pipeline_is_refused`, `on_failure_nesting_is_bounded`
- AC-2: `a_mutation_names_one_concrete_id`, `an_id_selects_one_or_all`, `a_get_answers_an_object_of_ids`
- AC-3: `a_pipeline_is_stored_read_listed_and_removed`
- AC-4: `what_the_node_cannot_run_is_refused_and_stores_nothing`
- AC-5: `a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed`
- AC-6: `the_pipeline_routes_are_served_only_when_pipelines_are_on`
