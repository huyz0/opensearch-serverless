# Verified: a writer node's shard cap

- AC-1: `no_cap_opens_every_shard_and_evicts_none` in crates/node/tests/writer_shard_cap.rs
- AC-2: `at_the_cap_an_idle_shard_goes_to_open_the_next` in crates/node/tests/writer_shard_cap.rs
- AC-3: `at_the_cap_with_every_shard_in_use_the_open_is_refused_and_counted` in crates/node/tests/writer_shard_cap.rs
- AC-4: `eviction_off_means_the_cap_only_refuses` in crates/node/tests/writer_shard_cap.rs
- AC-5: `an_evicted_shard_keeps_its_writes_and_is_taken_again_on_its_next_write` in crates/node/tests/writer_shard_cap.rs
- AC-6: `a_head_that_cannot_be_given_back_leaves_the_shard_asleep_at_its_term` in crates/node/tests/writer_shard_cap.rs
  and `an_evicted_shard_keeps_its_writes_and_is_taken_again_on_its_next_write` in crates/node/tests/writer_shard_cap.rs
- AC-7: `the_shard_cap_is_off_by_default_and_set_by_its_variables` in crates/node/src/boot/config/tests.rs
- Request budget: `evicting_a_published_shard_costs_one_put_and_a_shard_with_unpublished_writes_one_more` and `at_the_cap_with_every_shard_in_use_the_open_is_refused_and_counted` in crates/node/tests/writer_shard_cap.rs
