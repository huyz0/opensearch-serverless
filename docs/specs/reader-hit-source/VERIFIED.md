# Reader: _source filtering and track_total_hits: verified

Run with `cargo test -p oss-reader -p oss-compat` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container); the new fixture steps and the filtered sources recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `each_filter_writes_what_opensearch_wrote`, `numbers_print_as_java_prints_them`
- AC-3: `bad_filters_fail_as_opensearch_fails`, `whole_sources_and_none_are_not_filtered`, `a_source_that_is_not_an_object_is_an_error`
- AC-4: `keys_iterate_in_java_hash_map_order`
