# Verified: node-boot-tail

- AC-1: `a_node_with_the_tail_overlay_on_starts_and_serves_in_every_visibility` in crates/node/tests/boot.rs
- AC-2: `a_node_starts_with_the_tail_overlay_off_unless_the_setting_is_on` in crates/node/tests/boot.rs
- AC-3: `the_tail_overlay_is_off_and_visibility_everywhere_unless_set` in crates/node/src/boot/config.rs
- AC-4: `both_tail_actions_are_listed_and_refuse_until_their_handlers_are_given` in crates/node/src/boot/tail.rs
