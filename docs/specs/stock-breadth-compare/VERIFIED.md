# Stock OpenSearch comparison, breadth: verified

Run on 2026-11-27 (Linux cloud container, provisional). Stock OpenSearch 3.10.0 (build b9f09249, the release tarball from `artifacts.opensearch.org`, SHA-512 checked, security plugin removed) ran as a non-root user, single node, 1 GiB heap. `python3 fixtures/compat/record.py` recorded the fixtures from it; the replay ran on this branch.

- AC-1: `an_assembled_node_answers_every_breadth_fixture_as_stock_opensearch_did` (crates/compat/tests/node_cluster.rs): 32 query steps. terms, exists, prefix, wildcard, regexp, fuzzy, ids, match_none, match_phrase, multi_match (best_fields, most_fields, phrase), query_string, simple_query_string, constant_score, dis_max, bool, track_total_hits and _source filtering, scores included.
- AC-2: `an_assembled_node_answers_every_breadth_fixture_as_stock_opensearch_did` (crates/compat/tests/node_cluster.rs): 22 aggregation steps. terms (keyword and integer, order, size, min_doc_count, include/exclude, missing, metric sub-aggregations excepted), histogram, date_histogram, keyed range, filters, missing, global, composite with after, cardinality, and three refusals.
- AC-3: `recorded_fixtures_replay_cleanly` (crates/compat/tests/recorded.rs) parses and replays `fixtures/compat/search-breadth-gaps/queries.json` and `fixtures/compat/search-breadth-gaps/bucket-aggs.json` against their own recordings. search-breadth-gaps/ holds the stock answers for 13 steps the node refuses with a 400 "not supported by this node yet": prefix with case_insensitive, match_phrase_prefix, match_bool_prefix, match with fuzziness, multi_match cross_fields, boosting, any aggregation over a double field (terms with avg/sum, histogram with offset, range, filter with sum), and format on date_histogram and date_range. Not replayed.

Findings:

- Stock puts `"terminated_early": true` in a `size: 0` search with aggregations and no `terminate_after`; this node does not. The aggregation fixtures send `size: 1` so the comparison covers the aggregations; the `size: 0` field is a difference left open.
- Three error steps normalise a column or node id (the replay sends keys sorted; the node id differs).

Not run: the `min` fixture (recording refused by the sandbox), nested-aggregation `terms` ordering, a multi-shard or multi-segment comparison.

R10.1 and R10.2 stay open: 13 recorded answers are not matched yet.
