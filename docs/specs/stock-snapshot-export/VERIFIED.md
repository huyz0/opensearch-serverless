# Verified: stock-snapshot-export

- AC-1: `every_recorded_json_round_trips`, `integers_of_every_width_and_strings_of_every_class_round_trip`, `a_double_round_trips_by_value`, `the_bounds_hold_at_their_limit_and_one_past`, `not_json_and_a_big_integer_are_refused_and_the_buffer_keeps_its_start` pass. Jackson 3.2.3 reads the encoder's output for the nine recorded cases it can express to the same tree (`fixtures/snapshots/stock-smile/CheckSmileWrite.java`, by hand, 9 cases, 0 different). The tests compiled red first (the function did not exist), then passed together; no test was seen failing on a wrong result.
- AC-2: `a_written_blob_reads_back_at_every_body_length`, `the_header_names_the_codec_at_version_one_and_the_footer_sums_it`, `another_codec_is_refused_on_read`, `a_failed_write_leaves_the_buffer_as_it_was`, `a_codec_name_too_long_for_a_header_is_refused` pass. Lucene 10.5.1's `CodecUtil` (`checkHeader` at version 1, `checkFooter`, `checksumEntireFile`, `retrieveChecksum`) accepts four written blobs (`fixtures/snapshots/stock-envelope/CheckStockWrite.java`, by hand against the 3.10 node's jars, 4 of 4 ok). The tests compiled red first (the function did not exist); the codec-name test then failed on a real defect (a 128-byte name has a two-byte length), fixed by refusing 128 and over. The two writers' 39 mutants are all caught.
- AC-3: `the_recorded_index_blob_is_written_back_byte_for_byte`, `a_snapshot_of_a_new_index_is_added_beside_the_recorded_one`, `a_second_snapshot_of_a_held_index_joins_it_with_its_new_generations`, `a_held_name_under_another_id_is_refused`, `a_repeated_snapshot_name_or_an_unsafe_id_is_refused`, `an_empty_repository_starts_from_the_first_snapshot` pass. The server's `RepositoryData.snapshotsFromXContent` (3.10) parses three written `index-N` files (an empty start, a new index added, a held index joined) in the same Java check, 3 of 3 ok. The generation number of the next `index-N` is chosen by the take (AC-6), not by this writer. The tests compiled red first.
- AC-4: NOT-RUN: the metadata and shard snapshot writers are not built yet.
- AC-5: NOT-RUN: the copy and the target store are not built yet.
- AC-6: NOT-RUN: the take and its fencing are not built yet.
- AC-7: NOT-RUN: the take's refusals are not built yet.
- AC-8: NOT-RUN: the export allow-lists and the target store are not built yet.
- AC-9: NOT-RUN: the switch and the route are not built yet.
- AC-10: NOT-RUN: the budget rows wait for the take.
- AC-11: NOT-RUN: needs the take; the stock 3.10 build is at hand (`opensearch-min`, run as a non-root user).
