# Verified: reader warm copy on join

- AC-1: partly: `crates/node/src/boot/config/page_cache.rs` tests (`the_warm_peer_is_off_unless_asked_for_and_then_has_its_defaults`, `warming_without_a_disk_tier_is_refused_and_a_tier_alone_is_not_warm`) and `crates/node/src/boot/tail.rs` (`both_warm_actions_are_listed_and_refuse_until_their_handlers_are_given`); the disk tier itself is `docs/specs/node-page-cache-disk`. NOT-RUN: a booted node answering the actions over the transport, which the two-node test of AC-5 covers
- AC-2: partly: `crates/node/src/warm_handlers/tests.rs` (`a_joiner_is_listed_only_the_shards_it_is_among_the_first_readers_of`, `a_joiner_the_leases_do_not_show_is_placed_as_if_it_were_a_reader`, `the_node_limits_win_over_a_bigger_ask`) and `crates/node/src/warm_peer/tests.rs` (pages most read first under the byte total, pinned and decoded files left out). NOT-RUN: the per-index off, frozen and unopened shard cases; the node lists only open shards, and no per-index setting is read yet
- AC-3: partly: `crates/node/src/warm_handlers/tests.rs` (`pages_are_served_for_held_shards_and_a_key_elsewhere_is_refused`, `a_node_serving_its_limit_refuses_the_next_request_and_takes_one_again_when_one_ends`, `a_handler_at_its_limit_answers_busy`) and `warm_peer/tests.rs` (a page not held is skipped). NOT-RUN: the peer's metered object-store request count over a batch, and the page-size mismatch refusal (joiner side)
- AC-4: NOT-RUN: not built yet
- AC-5: NOT-RUN: not built yet
- AC-6: NOT-RUN: not built yet
- AC-7: NOT-RUN: not built yet
- AC-8: NOT-RUN: not built yet
- AC-9: NOT-RUN: not built yet
