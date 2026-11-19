# Verified: the coordinator reduces each named metric aggregation across shards

- AC-1: `a_name_every_shard_answered_reduces_over_all_of_them` in crates/coordinator/tests/agg_reduce.rs
- AC-2: `a_name_only_some_shards_have_reduces_over_those` in crates/coordinator/tests/agg_reduce.rs
- AC-3: `names_come_out_once_in_first_seen_order` in crates/coordinator/tests/agg_reduce.rs
- AC-4: `values_of_different_kinds_are_refused` in crates/coordinator/tests/agg_reduce.rs
- AC-5: `top_hits_is_refused_for_the_piece_that_fetches_hits` in crates/coordinator/tests/agg_reduce.rs
- AC-6: `no_shards_or_no_aggregations_give_none` in crates/coordinator/tests/agg_reduce.rs
