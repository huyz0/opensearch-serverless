# Verified: the reader builds an overlay segment from pushed writes

- AC-1: `records_at_or_below_the_checkpoint_are_dropped_and_the_highest_sequence_number_wins` in crates/reader/tests/it/overlay_segment.rs
- AC-2: `an_index_record_is_a_document_and_a_deletion_is_none` in crates/reader/tests/it/overlay_segment.rs
- AC-3: `ids_are_each_replaced_or_deleted_id_once_in_first_seen_order` in crates/reader/tests/it/overlay_segment.rs
- AC-4: `more_than_the_most_ids_is_refused_and_exactly_the_most_builds` in crates/reader/tests/it/overlay_segment.rs
- AC-5: `a_source_the_mapping_refuses_names_its_id` in crates/reader/tests/it/overlay_segment.rs
- AC-6: `nothing_above_the_checkpoint_is_an_empty_overlay` in crates/reader/tests/it/overlay_segment.rs
- AC-7: `the_estimate_bounds_what_a_build_takes` in crates/reader/tests/overlay_heap.rs (provisional, WSL2-class numbers: the estimate was 1.5 to 2.3 times the measured peak across the shapes)
- AC-8: `the_estimate_grows_with_the_records_and_bounds_the_build` in crates/reader/tests/it/overlay_segment.rs
