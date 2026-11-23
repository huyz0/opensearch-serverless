# Segment publish: verification

- AC-1: `the_javas_segment_header_is_re_encoded_byte_for_byte`, `a_header_is_settled_when_the_offsets_gain_a_digit`, `a_header_refuses_what_a_blob_cannot_hold` (format, `tests/writers.rs`).
- AC-2: `the_javas_manifest_re_encodes_to_the_same_commit`, `locations_encode_to_the_text_they_parse_from` (format, `tests/writers.rs`).
- AC-3: `a_new_segment_is_one_blob_and_one_swap_with_no_reads_after_the_first` (writer, `tests/publish.rs`).
- AC-4: `only_fresh_files_are_uploaded_and_inherited_locations_are_kept` (writer, `tests/publish.rs`).
- AC-5: `a_taken_blob_name_gets_a_unique_one` (writer, `tests/publish.rs`).
- AC-6: `a_newer_term_fences_an_older_one_and_a_second_writer_of_a_term_is_foreign` (writer, `tests/publish.rs`).
- AC-7: `a_lost_swap_reads_the_register_once_and_publishes_again`, `a_manifest_over_the_first_read_buffer_is_read_whole` (writer, `tests/publish.rs`).
- AC-8: `a_64_mib_segment_publishes_with_peak_heap_below_2_mib` (writer, `tests/publish_memory.rs`, counting allocator, the segment on disk and mapped).
- AC-9: `the_reader_opens_what_the_writer_published` (node, `tests/roles/publish_read.rs`).
