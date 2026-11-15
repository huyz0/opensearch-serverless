# Streaming JSON tokenizer (scalar stage): verified

- AC-1: `events_rebuild_what_serde_json_parses` (2,000 cases, chunk sizes 1, whole and random)
- AC-2: `accepts_exactly_what_serde_json_accepts`, `arbitrary_short_inputs_agree_with_serde_json`, `numbers_outside_f64_range_pass_through_as_text`; the fuzz target `crates/chunkjson/fuzz/fuzz_targets/oracle.rs` ran 6.9M inputs for 240 s with no disagreement (nightly, not in the gate)
- AC-2a: `concatenated_framing_agrees_with_serde_json_streams` (fails with adjacent-scalar refusal disabled), `adjacent_scalars_are_one_bad_document_not_two`
- AC-3: `surrogate_escapes_must_pair_in_order`, `invalid_utf8_is_refused_where_it_breaks`, `split_character_broken_after_the_slice_names_the_breaking_byte`, `control_characters_and_bad_escapes_are_refused`, `bad_numbers_are_refused`, `structure_errors_name_the_byte`, `truncated_input_and_trailing_data_are_refused`
- AC-4: `nesting_at_the_limit_is_accepted_and_one_more_refused`, `numbers_longer_than_the_limit_are_refused`
- AC-5: `a_64_mib_string_streams_in_64_kib_chunks_without_heap_growth` (with fragments copied to the heap it read 67,041,755 bytes of growth and failed)
- AC-6: `ndjson_documents_end_with_their_byte_ranges`, `ndjson_refuses_a_newline_inside_a_document`, `ndjson_and_concatenated_accept_empty_input`
- AC-7: `reader_matches_the_slice_api`, `every_buffer_type_reads`, `errors_pass_through`
- AC-8: `every_call_after_an_error_returns_the_same_error`
- AC-9: `scripts/check-layering.py` (refused the bench's former `oss-rest` path dependency before it was removed)
