# Reader: `histogram` and `date_histogram` ordered by a sub-aggregation: verified

Run on 2026-11-27 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java. The tests were written after the code, so none was seen failing first.

- AC-1: `an_order_by_a_path_parses_to_the_path_and_direction`, `a_bad_path_fails_as_it_does_for_a_terms`
- AC-2: `buckets_list_by_the_targets_value_in_either_direction`
- AC-3: `the_reduce_orders_by_the_reduced_values_and_gap_buckets_sort_last`
- AC-4: `a_target_the_shard_cannot_order_by_fails_it`
- AC-5: `a_date_histogram_is_ordered_the_same_way`
