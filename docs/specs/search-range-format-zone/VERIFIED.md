# Verified: a range's `format` and fixed `time_zone`

- AC-1: `a_range_reads_its_bounds_in_the_zone_and_format_it_names` in crates/node/tests/it/search_range_zone.rs (seen red as "[range] [time_zone] is not supported by this node yet", green after) and `a_zone_reads_a_bound_with_none_of_its_own_and_leaves_one_that_has_one` in crates/reader/src/date_math_tests.rs
- AC-2: `rounding_and_arithmetic_run_on_the_calendar_of_the_zone` in crates/reader/src/date_math_tests.rs and `a_range_reads_its_bounds_in_the_zone_and_format_it_names` in crates/node/tests/it/search_range_zone.rs
- AC-3: `a_forced_format_reads_only_its_own_text` in crates/reader/src/date_math_tests.rs, `a_range_reads_its_format_and_zone_and_refuses_what_it_cannot` in crates/reader/src/dsl/query_tests.rs and `a_range_refuses_a_format_or_zone_it_does_not_read` in crates/node/tests/it/search_range_zone.rs
