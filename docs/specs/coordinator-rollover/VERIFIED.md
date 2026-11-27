# Verified: the coordinator decides a rollover from the body and what the target says

- AC-1: `time_values_parse_and_print_as_core_does` in crates/coordinator/tests/it/time_value.rs
- AC-1 (refusals): `time_value_refusals_use_the_javas_words` in crates/coordinator/tests/it/time_value.rs
- AC-2: `the_next_name_increments_a_trailing_number` in crates/coordinator/tests/it/rollover.rs
- AC-3: `the_body_names_only_supported_object_fields_and_conditions` in crates/coordinator/tests/it/rollover.rs
- AC-4: `the_body_shapes_the_new_index_as_a_create_request` in crates/coordinator/tests/it/rollover.rs
- AC-5: `the_target_must_be_a_rollable_alias` in crates/coordinator/tests/it/rollover.rs
- AC-6: `conditions_are_evaluated_against_what_is_recorded` in crates/coordinator/tests/it/rollover.rs
