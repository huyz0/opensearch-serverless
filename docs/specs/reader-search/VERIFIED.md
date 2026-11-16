# Reader: `_search`: verified

Run with `cargo test --workspace --all-features` and `scripts/gate.sh` on 2026-11-15 (Linux cloud container).

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `an_index_opens_once_with_two_gets_and_no_list`
- AC-3: `an_idle_index_makes_no_requests`
- AC-4: `a_missing_index_is_not_found_after_one_get`
- AC-5: `a_bad_body_is_refused_before_the_index_is_opened`
- AC-6: `shard_failures_name_the_node_and_the_index`
- AC-7: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the window error step of `errors.json`), `a_search_the_request_body_pool_cannot_hold_is_refused_with_429` (seen red with the reservation removed, and with keyword terms left out of it), `the_visitor_keeps_only_the_id_and_the_source`, `a_refused_reservation_copies_nothing_and_is_kept`
- AC-8: `a_search_streams_hits_with_their_source`
- AC-9: `concurrent_large_bodies_and_windows_stay_within_the_pool`
- AC-10: `an_index_the_node_cannot_serve_is_refused`, `a_node_whose_pool_cannot_hold_the_server_buffers_does_not_start`; the open-index limit and concurrent first opens are now [reader-budgets](../reader-budgets/VERIFIED.md) AC-6 and AC-8 (`a_full_pool_closes_the_least_recently_searched_index`, `concurrent_first_searches_open_once_and_close_nothing_else`)
- AC-11: `an_index_opens_once_with_two_gets_and_no_list`
