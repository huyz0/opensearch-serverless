# Reader: `date_range` aggregation: verified

Run on 2026-11-27 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java. The tests were written with the code; `bad_bodies_and_unserved_options_fail_by_name` was seen failing first, on the refusal of a bound with date math being wrapped out of sight.

- AC-1: `text_bounds_are_dates_and_numbers_are_milliseconds`, `bad_bodies_and_unserved_options_fail_by_name`
- AC-2: `each_range_counts_the_documents_between_its_dates`
- AC-3: `each_range_counts_the_documents_between_its_dates`, and the answer a date field's range writes, held by `a_date_range_also_writes_its_bounds_as_text`
- AC-4: `a_format_prints_the_bounds_and_the_keys` and the refusals in `bad_bodies_and_unserved_options_fail_by_name` (unit); the stock 3.10.0 answer replays in `an_assembled_node_answers_the_recorded_gaps_as_stock_did_except_the_open_ones` (crates/compat/tests/node_cluster.rs), run on 2026-11-27 (Linux cloud container, provisional).
