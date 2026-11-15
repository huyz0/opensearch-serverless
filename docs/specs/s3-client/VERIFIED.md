# S3 client: verified

Run with `cargo test -p oss-blob --all-features` and the whole gate, `scripts/gate.sh`, on 2026-11-15 (Linux sandbox, loopback; emulator numbers are provisional). The planted defects listed in SPEC.md's test plan were each applied alone and seen to fail their test.

- AC-1: `every_published_vector_signs_exactly` (20 suite vectors and the S3 GET Object example), `headers_are_sorted_by_name_and_repeats_kept_in_order`
- AC-2: `each_operation_is_one_attempt_of_its_kind_and_one_request`
- AC-3: `a_ranged_get_sends_its_range_and_gets_exactly_those_bytes`, `past_the_end_and_missing_keys_fail_in_one_attempt`
- AC-4: `a_put_declares_its_length_and_never_chunks`, `a_body_of_the_wrong_length_stores_nothing`, `bodies_need_a_declared_length`
- AC-5: `a_conditional_create_sends_if_none_match_and_never_overwrites`, `a_409_precondition_failed_is_taken_and_a_409_conflict_is_retried`, `of_racing_creates_over_http_exactly_one_wins`, `statuses_map_as_the_java_shell_maps_them`
- AC-6: `head_of_a_missing_key_is_none_and_delete_of_one_succeeds`
- AC-7: `listing_2500_keys_takes_three_pages_and_visits_each_once`, `a_page_is_the_next_keys_after_the_cursor_with_their_sizes`, `keys_with_xml_specials_decode`, `a_malformed_or_cut_list_page_is_refused`, `element_text_is_bounded`
- AC-8: `transient_statuses_are_retried_until_attempts_run_out`, `refusals_are_not_retried`, `injected_store_faults_are_retried_and_each_attempt_counted`, `a_connection_that_never_opens_is_retried_and_counted`, `every_pause_lies_within_its_bound`
- AC-9: `a_body_cut_part_way_is_unavailable_and_not_retried`, `a_body_cut_before_its_first_byte_is_retried`, `a_put_is_retried_only_when_its_body_rewinds`, `a_list_cut_after_its_first_key_is_not_retried`
- AC-10: `a_lost_response_to_a_create_is_retried_and_finds_its_own_object`
- AC-11: `a_request_signed_with_the_wrong_secret_is_denied_once`
- AC-12: `invalid_arguments_fail_before_any_request`
- AC-13: `a_16_mib_object_streams_over_http_through_one_pooled_buffer` (measured: PUT 16,857,521 bytes above baseline, 80,305 over the one stored copy; GET 5,880 bytes)
- AC-14: NOT-RUN: no RustFS binary could be run in this sandbox (starting the RustFS image, or a binary taken from it, was refused); `the_client_round_trips_against_rustfs` skips without `RUSTFS_BIN` and was run that way
- AC-15: `scripts/check-test-endpoints.py`, `scripts/check-alloc.py`, `scripts/check-s3-access.py`
