# Reader: `significant_terms` aggregation: verified

Run on 2026-11-27 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java (`JLHScore`, `PercentageScore`, `InternalSignificantTerms.reduce`, `SignificanceLookup`). The tests were written beside the code; two expectations (the default shard size, and that a shard keeps buckets of score 0) were wrong in the first run and were corrected to what the Java does, so the others were not seen failing first.

- AC-1: `the_body_parses_with_its_defaults_and_options`, `bad_and_unserved_options_fail`
- AC-2: `the_heuristics_score_as_java_does`
- AC-3: `a_shard_keeps_the_best_size_with_enough_documents`, `a_reduce_sums_shards_rescores_and_drops_scores_of_zero`
- AC-4: `a_query_makes_its_over_represented_terms_significant`, `the_default_minimum_and_a_query_of_everything_leave_no_bucket`
- AC-5: `sub_aggregations_and_nesting_are_refused`, `an_unmapped_field_answers_nothing_and_a_numeric_one_is_refused`
