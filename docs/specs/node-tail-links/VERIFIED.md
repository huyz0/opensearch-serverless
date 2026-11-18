# Verified: the tail's four transport links

- AC-1: `a_push_reaches_the_reader_which_holds_the_shard_it_tracks_and_names_the_rest` in crates/node/tests/tail_links.rs
- AC-2: `a_push_that_cannot_be_delivered_or_verified_is_a_link_error_and_holds_nothing` in crates/node/tests/tail_links.rs
- AC-2 (bound, process): `a_push_over_the_bound_or_to_another_process_is_a_link_error` in crates/node/tests/tail_links.rs
- AC-3: `a_fetch_reaches_the_writer_signed_and_the_answer_grants_the_shard` in crates/node/tests/tail_links.rs
- AC-4: `a_fetch_to_a_writer_not_shown_live_or_one_that_does_not_answer_gives_nothing` and `a_writer_that_is_silent_fails_or_answers_garbage_is_no_answer` in crates/node/tests/tail_links.rs
- AC-5: `the_push_handler_refuses_a_message_that_does_not_decode` and `the_fetch_handler_refuses_a_message_that_does_not_decode_and_a_shard_it_does_not_write_gets_no_grant` in crates/node/tests/tail_links.rs
- AC-5 (bounds): `a_handler_refuses_a_message_over_its_bound` in crates/node/tests/tail_links.rs
