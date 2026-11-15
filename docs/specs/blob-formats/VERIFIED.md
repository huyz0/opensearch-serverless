# Shell blob format readers: verified

- AC-1: `descriptor_names_the_uuid_and_shards`
- AC-2: `manifest_reads_every_field`
- AC-3: `resolved_extents_hold_whole_lucene_files`, `a_reader_walks_from_the_index_name_to_its_files`
- AC-4: `segment_blob_header_agrees_with_the_manifest`
- AC-5: `node_pack_blocks_hold_segment_blobs_the_manifest_points_into`
- AC-6: `every_prefix_of_a_segment_blob_header_is_truncated`, `every_prefix_of_a_node_pack_header_is_truncated`, `every_prefix_of_a_manifest_or_descriptor_is_refused`
- AC-7: `single_bit_flips_never_panic`
- AC-8: `node_pack_prefix_damage_is_named`, `manifest_damage_is_refused`, `locations_follow_the_java_rules`, `descriptor_records_are_told_apart`, `segment_blob_damage_is_refused`, `json_strings_decode_escapes`
- AC-9: `segment_blob_header_length_limit`, `segment_blob_file_count_limit`, `node_pack_header_length_limit`, `node_pack_block_count_is_bounded_by_the_header`, `manifest_file_count_limit`, `json_nesting_limit`
- AC-10: `keys_carry_the_base`, `keys_refuse_parts_that_would_escape_or_collide`
- AC-11: `version_one_packs_carry_no_index_name`, `pack_strings_are_modified_utf8`
- AC-12: `records_a_reader_cannot_follow_say_so`, `a_reader_walks_from_the_index_name_to_its_files`
- AC-13: `scripts/gate.sh`
- AC-14: `skipped_escaped_strings_are_not_decoded`
