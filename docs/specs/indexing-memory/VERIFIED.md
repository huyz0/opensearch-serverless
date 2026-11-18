# Indexing memory: verification

- AC-1: `a_percentage_is_a_share_of_the_heap_with_a_floor_and_a_ceiling`, `the_controller_names_its_budget` (writer, `tests/indexing_memory.rs`).
- AC-2: `a_check_is_due_when_a_thirtieth_of_the_budget_has_been_written`, `racing_writers_ask_for_no_more_checks_than_the_bytes_allow` (writer, `tests/indexing_memory.rs`).
- AC-3: `under_budget_refreshes_nothing`, `over_budget_refreshes_the_largest_until_the_rest_fits`, `a_shard_with_nothing_buffered_is_never_refreshed` (writer, `tests/indexing_memory.rs`).
- AC-4: `bytes_being_written_are_not_counted_twice` (writer, `tests/indexing_memory.rs`).
- AC-5: `past_one_and_a_half_budgets_the_refreshed_are_throttled_once`, `easing_below_one_and_a_half_budgets_releases_every_throttled_shard` (writer, `tests/indexing_memory.rs`).
- AC-6: `a_shard_idle_for_the_inactive_time_is_flushed` (writer, `tests/indexing_memory.rs`).
- AC-7: `a_shard_is_idle_since_its_last_write` (writer, `tests/shard.rs`).
