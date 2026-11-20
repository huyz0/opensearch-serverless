# Verified: the cluster half of the writer's routes

- AC-1: `the_winner_is_local_only_on_its_own_node` in crates/node/tests/cluster_owner.rs
- AC-2: `dead_and_non_ingest_members_are_never_chosen` in crates/node/tests/cluster_owner.rs
- AC-3: `a_group_is_one_request_and_each_outcome_is_the_items` in crates/node/tests/cluster_owner.rs
- AC-4: `a_group_is_one_request_and_each_outcome_is_the_items` in crates/node/tests/cluster_owner.rs
- AC-5: `a_gone_owner_a_large_request_and_a_short_answer_fail_their_items` in crates/node/tests/cluster_owner.rs
- AC-6: `an_update_or_bad_utf8_fails_alone_and_is_not_sent` in crates/node/tests/cluster_owner.rs
