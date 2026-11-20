# Verified: a node searches a point in time

- AC-1: `a_search_of_a_view_answers_what_a_live_search_does` in crates/node/tests/it/frozen_search.rs
- AC-2: `a_search_of_a_view_does_not_read_the_live_commit` in crates/node/tests/it/frozen_search.rs
- AC-3: `a_second_search_of_a_view_reads_no_index_and_holds_one_entry` in crates/node/tests/it/frozen_search.rs
- AC-4: `an_unknown_released_or_expired_view_is_missing` in crates/node/tests/it/frozen_search.rs
- AC-5: `keep_alive_moves_the_deadline_and_over_an_hour_is_refused` in crates/node/tests/it/frozen_search.rs
- AC-6: `what_cannot_be_served_is_refused_before_any_read` in crates/node/tests/it/frozen_search.rs
- AC-7: `a_pool_without_room_for_the_view_buffer_answers_429_first` in crates/node/tests/it/frozen_search.rs
