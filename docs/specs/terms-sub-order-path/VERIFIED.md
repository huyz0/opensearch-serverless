# Verified: terms ordered by a path through single-bucket aggregations

Run on 2026-11-27 (Linux cloud container, provisional). Stock OpenSearch 3.10.0 (the release tarball from `artifacts.opensearch.org`, SHA-512 checked) recorded `fixtures/compat/nested-search/terms-sub-order.json` with `fixtures/compat/record.py`; the replay ran on this branch.

- AC-1: `an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did` (crates/compat/tests/node_cluster.rs), the numeric terms steps ordered by a filter and a missing.
- AC-2: the same test (`an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did`), the keyword terms by nested steps and the terms under nested by reverse_nested steps.
- AC-3: the same test, the steps that fail; `a_path_that_leads_nowhere_is_refused_as_stock_refuses_it` (unit).
- AC-4: `a_bucket_sorts_by_the_value_below_its_single_bucket_aggregations`, `an_empty_bucket_sorts_as_its_aggregations_say`, `a_path_through_single_bucket_aggregations_is_accepted` (unit).
- AC-5: `fixtures/compat/search-metrics-gaps/terms-bucket-subs.json`, parsed and replayed against its own recording by `recorded_fixtures_replay_cleanly`, not against the node.

The replay and the unit tests were written with the code and not seen red.
