# Mappings: verification

- AC-1: `mapping_steps_replay` (writer, `tests/mapping_fixtures.rs`, over `fixtures/compat/mapping/*.json`); `meta_limits_are_javas`, `integer_parameters_read_as_java_reads_them`, `number_null_values_keep_javas_range_rules` (writer, `tests/mapping_edges.rs`).
- AC-2: `unsupported_types_and_parameters_are_refused_as_unsupported`, `default_valued_unsupported_parameters_are_accepted` (writer, `tests/mapping.rs`).
- AC-3: `a_hash_ordered_mapping_reads_as_the_same_mapping` (writer).
- AC-4: `total_fields_and_depth_limits` (writer).
- AC-5: `malformed_json_and_duplicates_are_refused`, `routing_and_detection_merge` (writer).
- AC-6: `mapping_steps_replay` (writer, `tests/mapping_fixtures.rs`, over the document steps of `fixtures/compat/mapping/*.json`, including `values.json` and `document-edges.json`).
- AC-7: `each_type_reads_its_values`, `dynamic_fields_and_their_update`, `dynamic_modes`, `refusals_outside_the_fixtures` (writer, `tests/document.rs`).
- AC-8: `the_total_fields_limit_holds_while_fields_are_added` (writer, `tests/document.rs`).
