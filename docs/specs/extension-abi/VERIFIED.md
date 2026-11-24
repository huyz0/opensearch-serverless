# Verified: extension modules, the ABI

- AC-1: `a_module_is_accepted_for_the_same_major_and_a_minor_at_most_the_hosts` in crates/ext/tests/abi.rs
- AC-2: `a_well_formed_descriptor_is_read_whole`, `an_unknown_duplicate_or_missing_key_is_refused_naming_the_key`, `a_wrong_type_or_value_is_refused_naming_the_key`, `a_duplicate_point_is_refused`, `a_descriptor_over_the_ceiling_or_that_is_not_json_is_refused`, `nesting_is_bounded_so_a_hostile_descriptor_cannot_exhaust_the_stack` and `a_string_escape_is_read_as_json_reads_it`, `numbers_are_whole_non_negative_and_fit_or_the_key_is_refused` and `characters_of_every_width_are_read_whole` in crates/ext/tests/descriptor.rs
- AC-3: `zero_is_success_every_failure_is_negative_and_anything_else_is_malformed`, `a_failure_names_its_kind_and_the_http_status_it_answers` and `a_module_s_detail_is_bounded_and_cannot_forge_a_log_line` in crates/ext/tests/abi.rs
- AC-4: `the_ceilings_are_the_ones_the_spec_names` in crates/ext/tests/abi.rs; `optional_limits_may_only_ask_for_less` in crates/ext/tests/descriptor.rs
