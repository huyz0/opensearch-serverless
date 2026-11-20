# Verified: `_field_caps`

- AC-1: `every_mapped_field_is_listed_with_its_capabilities` in crates/node/tests/it/field_caps.rs
- AC-2: `fields_takes_names_and_patterns_and_leaves_out_what_matches_nothing` in crates/node/tests/it/field_caps.rs
- AC-3: `index_doc_values_and_fielddata_decide_searchable_and_aggregatable` in crates/node/tests/it/field_caps.rs
- AC-4: `several_indices_merge_by_type_and_name_the_indices_that_differ` in crates/node/tests/it/field_caps.rs
- AC-5: `a_call_without_fields_and_an_unknown_parameter_are_refused` in crates/node/tests/it/field_caps.rs
- AC-6: `an_unknown_index_is_not_found_and_a_pattern_is_refused` in crates/node/tests/it/field_caps.rs
- AC-7: `an_index_of_two_shards_answers_as_one_of_one` in crates/node/tests/it/field_caps.rs
- AC-8: `get_and_post_answer_alike_and_a_warm_call_makes_no_request` in crates/node/tests/it/field_caps.rs
