# GET /{index}/_mapping on the node: verification

- AC-1: `a_get_answers_the_stored_mapping_under_the_index_name` (node, `tests/server/index_route/mapping_get.rs`).
- AC-2: `an_index_with_no_mapping_answers_an_empty_one` (node, `tests/server/index_route/mapping_get.rs`).
- AC-3: `what_is_not_an_index_is_refused_before_it_costs_more_than_a_read` (node, `tests/server/index_route/mapping_get.rs`).
- AC-4: `a_range_query_on_a_dynamically_mapped_field_finds_every_document_on_every_node` (node, `tests/dynamic_mapping.rs`).
- AC-5: `a_get_is_one_read` (node, `tests/server/index_route/mapping_get.rs`).
- AC-6: `the_parameters_the_java_consumes_are_accepted` (node, `tests/server/index_route/mapping_get.rs`).
