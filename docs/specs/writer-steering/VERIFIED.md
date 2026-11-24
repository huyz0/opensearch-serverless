# Verified: a write for an unheld shard goes to a member with room

- AC-1: `a_write_for_an_unheld_shard_goes_to_the_member_with_room_when_this_node_is_nearly_full` in crates/node/tests/writer_hand_off.rs
- AC-2: `a_held_shard_is_never_steered_away` in crates/node/tests/writer_hand_off.rs
- AC-3: `a_write_is_not_steered_with_the_setting_off_or_a_node_with_room_or_no_member_with_room` in crates/node/tests/writer_hand_off.rs
- AC-4: `a_steered_shard_keeps_going_to_the_same_member_so_its_refresh_follows_its_write` in crates/node/tests/writer_hand_off.rs
- AC-5: `a_shard_the_routes_give_another_node_is_not_steered` in crates/node/tests/writer_hand_off.rs
