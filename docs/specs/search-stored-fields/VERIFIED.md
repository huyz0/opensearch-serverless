# Verified: the `stored_fields` key of a search body

- AC-1: `stored_fields_none_drops_the_id_and_the_source` in crates/reader/src/dsl/source_tests.rs and `none_drops_the_id_and_the_source_and_names_drop_the_source` in crates/node/tests/it/search_stored_fields.rs
- AC-2: `stored_fields_names_keep_the_id_and_drop_the_source_unless_asked_for` in crates/reader/src/dsl/source_tests.rs and `none_drops_the_id_and_the_source_and_names_drop_the_source` in crates/node/tests/it/search_stored_fields.rs
- AC-3: `an_object_is_not_a_leaf` and `a_stored_field_is_refused_by_name_or_pattern` in crates/reader/src/fetch/stored_fields_tests.rs, `stored_fields_and_objects_are_recorded` in crates/reader/src/mapping_tests.rs, and `what_the_node_cannot_answer_is_refused` in crates/node/tests/it/search_stored_fields.rs
