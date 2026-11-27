# Verified: a range's region `time_zone`

- AC-1: `a_region_reads_wall_clock_time_at_the_offset_in_force` in crates/reader/src/date_zone_tests.rs (seen red, the bound read as UTC, then green) and `a_range_reads_its_bounds_in_the_zone_and_format_it_names` in crates/node/tests/it/search_range_zone.rs
- AC-2: `a_region_day_is_as_long_as_the_calendar_makes_it` in crates/reader/src/date_zone_tests.rs
- AC-3: `a_gap_shifts_forward_and_an_overlap_takes_the_earlier_offset` in crates/reader/src/date_zone_tests.rs
- AC-4: `only_a_region_the_database_spells_is_read` in crates/reader/src/date_zone_tests.rs and `a_range_refuses_a_format_or_zone_it_does_not_read` in crates/node/tests/it/search_range_zone.rs
