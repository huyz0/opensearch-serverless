# Template resolution: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-coordinator --test templates`, `cargo clippy -p oss-coordinator --all-targets -- -D warnings`.

The expectations come from reading the Java source, not a recorded run (SPEC, test plan).

- AC-1: `no_template_means_nothing_inherited`, `patterns_match_the_whole_name_with_stars_as_the_only_wildcard`
- AC-2: `the_highest_priority_wins_and_a_tie_goes_to_the_smaller_name`
- AC-3: `a_priority_is_read_as_core_reads_a_long`
- AC-4: `a_matching_template_gives_its_settings_and_mappings`, `components_apply_in_order_and_the_template_on_top`, `a_field_replaces_the_one_before_unless_both_are_objects`, `arrays_and_scalars_are_replaced_not_merged`
- AC-5: `a_component_that_does_not_exist_is_reported`, `a_stored_template_that_cannot_be_read_is_reported_matching_or_not`
- AC-6: `a_data_stream_template_says_so_and_names_its_timestamp_field`, `many_indices_each_get_their_winning_template_name`
