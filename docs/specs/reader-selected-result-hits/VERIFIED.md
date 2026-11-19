# Verified: the reader writes the selected hits of a shard's top-level result

- AC-1: `selected_result_hits_are_written_as_the_response_writes_them` in crates/reader/src/response_selected_tests.rs
- AC-2: `selected_result_hits_carry_explain_sort_fields_and_source_filtering` in crates/reader/src/response_selected_tests.rs
- AC-3: `no_position_or_one_beyond_the_result_hits_is_refused` in crates/reader/src/response_selected_tests.rs
