# Reader: the percentiles and percentile_ranks aggregations: verified

Run with `cargo test -p oss-reader`, `cargo test -p oss-compat --test node_search` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container). Recordings from a stock OpenSearch 3.10.0-SNAPSHOT built from upstream's `3.10` branch; the vectors of AC-3 and the values of AC-4 printed by its `TDigestState`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the percentiles steps of `paging-sort.json`)
- AC-2: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the percentiles steps of `errors.json`), `malformed_options_fail_as_opensearch_fails_them`, `options_the_node_does_not_serve_are_refused`
- AC-3: `quantiles_cdfs_and_reduces_match_opensearch`, `percentiles_reduce_as_opensearch_reduces_them`
- AC-4: `percentiles_follow_the_digest_opensearch_builds`
- AC-5: `a_percentiles_reserves_its_digest_and_keys`, `a_digest_reserves_every_array_it_holds`, `a_digest_never_holds_more_than_it_reserves`
