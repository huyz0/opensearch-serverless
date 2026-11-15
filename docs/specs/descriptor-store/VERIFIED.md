# Descriptor store: verified

Run with `cargo test -p oss-format --test records`, `cargo test -p oss-meta` and the whole gate, `scripts/gate.sh`, on 2026-11-15 (Linux sandbox; OpenJDK and `jackson-core-3.2.3` for the recordings in `fixtures/jackson/`). Each listed defect was planted and seen to fail: `/` escaped by the writer (`strings_escape_as_jackson_escapes_them`, `every_field_writes_as_jackson_writes_it`), `mapping_version` and `settings_version` swapped in order (`every_field_writes_as_jackson_writes_it`), a create that reads first (`each_operation_sends_what_the_spec_budgets`, `create_over_a_tombstone_reads_once_and_swaps`), and a tombstone delete that skips its marker (`a_deleted_name_is_absent_and_can_be_created_again`, `each_operation_sends_what_the_spec_budgets`).

- AC-1: `the_shells_descriptor_writes_back_byte_for_byte`, `every_field_writes_as_jackson_writes_it`
- AC-2: `strings_escape_as_jackson_escapes_them`, `integers_print_as_java_longs`
- AC-3: `a_record_the_java_refuses_is_refused`, `unknown_fields_are_skipped`, `an_alias_or_a_tombstone_is_told_apart`, `a_record_the_java_would_not_build_is_not_written`
- AC-4: `tombstones_write_as_the_java_and_read_back`, `a_delete_files_under_its_utc_hour`
- AC-5: `each_rule_is_refused_with_the_javas_message`, `every_case_of_each_rule_is_refused`, `a_valid_name_passes`, `an_invalid_name_sends_nothing`
- AC-6: `create_then_get_returns_the_record_and_a_second_create_is_refused`, `update_applies_only_from_the_current_generation`, `a_deleted_name_is_absent_and_can_be_created_again`, `a_name_a_sweep_claimed_under_a_minute_ago_is_refused`, `an_alias_is_not_an_index_and_holds_its_name`, `delete_is_idempotent`, `a_malformed_register_is_an_error_not_absent`, `a_taken_name_longer_than_the_buffer_is_taken_not_an_error`, `a_name_a_later_rule_refuses_can_still_be_updated_and_deleted`
- AC-7: `each_operation_sends_what_the_spec_budgets`, `create_over_a_tombstone_reads_once_and_swaps`
- AC-8: `a_short_buffer_is_refused_with_the_length_needed_and_sends_nothing`, `a_short_buffer_says_how_long_one_must_be`
