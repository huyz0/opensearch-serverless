# Create inherits from its template: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-coordinator --test index_create_inherit`, `cargo clippy -p oss-coordinator --all-targets -- -D warnings`.

The expectations come from reading the Java source, not a recorded run (SPEC, test plan).

- AC-1: `nothing_inherited_changes_nothing`
- AC-2: `a_template_supplies_the_shard_count_unless_the_request_names_one`, `a_template_shard_count_the_create_would_refuse_is_refused`
- AC-3: `template_settings_sit_under_the_requests_and_structural_ones_are_dropped`
- AC-4: `template_mappings_sit_under_the_requests_and_a_field_replaces`, `a_mapping_that_is_not_one_object_is_refused`, `a_request_without_mappings_inherits_them_whole_and_aliases_pass_through`
