# Verified: a writer node hands idle shards to a member with room

- AC-1: `a_node_nearly_full_hands_off_one_idle_shard_to_a_member_with_room` in crates/node/tests/writer_hand_off.rs
- AC-2: `a_node_hands_off_nothing_when_every_member_is_full_too` in crates/node/tests/writer_hand_off.rs
- AC-3: `a_node_with_the_setting_off_hands_off_nothing` in crates/node/tests/writer_hand_off.rs
- AC-4: `a_node_under_its_share_hands_off_nothing_and_asks_the_store_nothing` in crates/node/tests/writer_hand_off.rs
- AC-5: `a_node_far_above_the_mean_hands_off_towards_it` in crates/node/tests/writer_hand_off.rs
- AC-6: `a_handed_off_shard_gives_its_head_back_and_reopens_with_its_documents` in crates/node/tests/writer_hand_off.rs
- AC-7: `the_load_a_lease_reports_is_what_was_last_set_and_unsaid_before` in crates/node/src/writer/lease.rs
  and `a_started_nodes_lease_reports_its_cap_and_what_it_holds` in crates/node/tests/server/boot.rs
- AC-8: `hand_off_is_off_by_default_and_set_by_its_variable` in crates/node/src/boot/config/tests.rs
