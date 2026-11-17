# Reader: the top_hits aggregation: verified

Run with `cargo test -p oss-reader`, `cargo test -p oss-compat --test node_search` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container). Recordings from a stock OpenSearch 3.10.0-SNAPSHOT built from upstream's `3.10` branch.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the top-hits steps of `paging-sort.json` and `match.json`)
- AC-2: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the top-hits steps of `errors.json`), `bad_options_fail_as_opensearch_fails_them`, `options_this_node_does_not_serve_are_refused_by_name`, `the_window_a_size_and_a_sort_are_checked_as_they_are_there`
- AC-3: `the_first_hits_in_score_order_with_an_exact_total`, `a_sort_of_its_own_orders_across_segments_and_reports_no_score`, `the_request_s_own_sort_paging_and_total_do_not_reach_it`, `hits_are_the_query_s_matches_only`, `a_top_hits_is_one_of_the_aggregations_in_request_order`
- AC-4: `a_top_hits_reserves_its_own_window_of_hits`, `the_response_and_its_top_hits_share_the_fetch_buffers`
- AC-5: `the_hits_of_the_response_and_its_top_hits_are_fetched_in_one_round`
