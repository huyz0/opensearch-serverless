# Verified: extended_stats

Run on 2026-11-27 (Linux cloud container, provisional). Stock OpenSearch 3.10.0 (the release tarball from `artifacts.opensearch.org`, SHA-512 checked) recorded `fixtures/compat/search-metrics/extended-stats.json` with `fixtures/compat/record.py`; the replay ran on this branch.

- AC-1: `an_assembled_node_answers_every_metric_fixture_as_stock_opensearch_did` (crates/compat/tests/node_cluster.rs): doubles, integers, sigma 0, 3 and "2.5", no match, one document (variance_sampling `"NaN"`), two equal values, an unmapped field, under a terms bucket, beside stats, with typed keys.
- AC-2: the same test, step `extended_stats on a date field`; `a_date_field_also_writes_every_value_as_text` in crates/reader/src/aggs/metric/extended_stats_tests.rs.
- AC-3: the same test, the negative sigma, unknown key, no field, text field and keyword field steps; `sigma_is_a_number_of_at_least_zero` (unit).
- AC-4: `options_this_node_does_not_serve_are_refused_by_name` (unit); the stock answers for missing and format are in `fixtures/compat/search-metrics-gaps/extended-stats.json`, parsed and replayed against their own recording by `recorded_fixtures_replay_cleanly`, not against the node.
- AC-5: `shards_reduce_to_the_state_of_one_shard_holding_every_value` (unit, exact values).

The replay was not seen red before the code (the aggregation was refused, so it could only fail); the unit tests were written with the code and not seen red.
