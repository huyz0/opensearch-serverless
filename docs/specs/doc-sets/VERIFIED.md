# Document sets: verified

Run on 2026-11-24 in a Linux sandbox:
- `cargo test -p oss-format --test doc_sets`
- `cargo test -p oss-meta`
- `cargo clippy -p oss-format -p oss-meta --all-targets -- -D warnings`

The marker's Java bytes come from reading `TemplateStore.writeMarker`, not from a recorded run (SPEC, test plan).

- AC-1: `the_javas_marker_reads_and_writes_the_same_bytes`, `entries_are_written_in_the_javas_tree_map_order`, `an_empty_marker_has_no_entries`, `a_missing_version_or_entries_reads_as_the_java_reads_it`, `keys_are_the_javas_register_map_paths`
- AC-2: `more_entries_than_the_cap_are_refused_in_both_directions`, `the_cap_of_entries_at_their_longest_fits_the_bound`, `a_short_buffer_is_refused`, `a_counter_from_before_the_register_held_entries_is_unsupported`, `a_marker_the_java_could_not_have_written_is_refused`, `a_key_is_refused_for_a_bad_base_or_content`
- AC-3: `a_document_is_put_read_replaced_and_deleted`, `the_javas_marker_and_content_read_as_a_set`, `require_absent_refuses_an_existing_name_and_keeps_the_document`, `a_full_set_refuses_a_new_name_and_still_replaces`
- AC-4: `request_counts_per_operation`, `a_bad_name_or_document_sends_nothing`
- AC-5: `a_put_retries_a_lost_swap_and_writes_its_bytes_once`, `a_put_that_loses_every_attempt_leaves_no_bytes_behind`, `a_put_that_finds_the_name_taken_on_its_retry_drops_its_bytes`
- AC-6: `a_document_the_marker_names_but_the_store_lost_is_reported`
