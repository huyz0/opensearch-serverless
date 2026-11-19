# Verified: a reader opens any shard of an index

- AC-1: `a_second_shard_opens_with_two_gets_and_is_kept_beside_the_first` in crates/node/tests/it/shards.rs
- AC-2: `a_shard_the_index_does_not_have_is_refused_and_opens_nothing` in crates/node/tests/it/shards.rs
- AC-3: `a_push_for_a_shard_past_the_first_reaches_that_shard_alone` in crates/node/tests/it/shards.rs
- AC-4: `an_index_the_node_cannot_serve_is_refused` in crates/node/tests/it/search.rs
