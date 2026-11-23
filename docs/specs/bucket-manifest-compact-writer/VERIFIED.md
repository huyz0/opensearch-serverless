# Verified

- AC-1: `every_codec_round_trips_and_the_member_reader_agrees`
- AC-2: `the_body_is_what_the_shell_writes_for_the_same_manifest` (uncompressed bytes equal the test-only encoder's; compressed bodies parse to the same manifest), `variable_length_integers_are_the_shells`
- AC-3: `parse_reads_every_codec_including_lz4`
- AC-4: `auto_compresses_only_a_large_manifest_that_shrinks`
- AC-5: `every_proper_prefix_of_a_compact_manifest_is_an_error`, `too_many_members_are_refused_on_read`, `an_unknown_codec_and_a_body_with_trailing_bytes_are_refused`, `a_string_past_1024_characters_is_refused_on_write`, `a_body_past_the_reader_cap_is_refused`
- AC-6: `only_a_canonical_range_is_taken_apart`, `a_node_pack_is_taken_apart_only_when_it_puts_back_the_same`
