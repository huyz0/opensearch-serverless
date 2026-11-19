# Verified: a node coordinates a search over its own shards

- AC-1: `two_shards_answer_as_one_with_each_hit_from_its_shard` in crates/node/tests/it/coordinate.rs
- AC-2: `the_sort_orders_the_merge` in crates/node/tests/it/coordinate.rs
- AC-3: `a_shard_the_index_lacks_fails_alone` in crates/node/tests/it/coordinate.rs
- AC-4: `a_refused_reserve_fails_the_search` in crates/node/tests/it/coordinate.rs
- AC-5: `the_total_follows_track_total_hits` in crates/node/tests/it/coordinate.rs
- AC-6: `the_search_route_answers_an_index_of_two_shards_through_the_coordinator` in crates/node/tests/it/coordinate.rs
- AC-7: `each_shard_reserves_its_own_hits_before_it_searches` and `shards_whose_hits_do_not_fit_fail_before_they_search` in crates/node/tests/it/coordinate.rs
