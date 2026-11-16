# Transport: evidence

- AC-1: `every_recorded_primitive_is_the_javas_both_ways`, `versions_mask_and_accept_only_the_recorded_one`.
- AC-2: `every_recorded_primitive_is_the_javas_both_ways` (every cut of every encoding), `malformed_values_are_refused`, `a_count_the_bytes_cannot_hold_is_refused_before_anything_is_sized`, `a_write_past_the_limit_is_refused_and_writes_nothing`, `reading_tracks_its_position`.
- AC-3: `every_recorded_node_is_the_javas_both_ways`, `roles_read_sorted_once_each_and_addresses_are_four_or_sixteen_bytes`.
- AC-4: `every_recorded_frame_encodes_and_decodes_as_the_javas`, `status_bits_are_the_javas`, `a_tcp_handshake_request_may_end_before_its_version_and_ignores_what_follows_it`, `a_transport_handshake_response_may_name_no_node`, `a_frame_past_its_limit_leaves_the_buffer_as_it_was`.
- AC-5: `a_ping_and_bad_prefixes`, `variable_header_lengths_are_checked`, `compressed_messages_inflate_to_a_bound`.
