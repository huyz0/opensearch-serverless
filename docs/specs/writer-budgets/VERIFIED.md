# Verified: writer-budgets

In-process emulator, request counts from the store's meter; nothing about time is claimed.

- AC-1: `the_first_take_of_four_never_written_shards_costs_what_the_table_says` in crates/node/tests/writer_budgets.rs: 16 requests, no LIST (Java: at most 50). Seen red: with the row at 17 the gate fails as improved.
- AC-2: `a_write_to_each_of_four_held_shards_then_a_publish_costs_what_the_table_says` in crates/node/tests/writer_budgets.rs: 34 requests, no LIST, 18 for the four writes and 16 for the four publishes (Java's row: 23).
- AC-3: `releasing_four_idle_shards_costs_what_the_table_says` in crates/node/tests/writer_budgets.rs: 8 requests, no LIST, four hibernated and four given back (Java: 43).
- AC-4: `an_hour_of_passes_after_four_indices_were_released_asks_for_nothing` and `an_hour_of_passes_over_idle_held_shards_asks_for_nothing` in crates/node/tests/idle_hour.rs: 0 requests an hour, shards still held in the held hour and none held after the release.

Not covered: lease renewal and the stored-script marker (`lease.idle.*`), RustFS counts for these rows, a timer outside the called passes, the reader's idle hour (`search.held_idle.index_requests_target` stays blocked).
