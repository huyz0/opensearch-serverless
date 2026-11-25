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

- Multi-node run (switch decision), `crates/node/tests/hand_off_fill_evidence.rs` (`--ignored`): two real nodes on the emulator with the Java latency profile, provisional, one machine. n1 holds the whole cap (20 shards of one 20-shard index, `OSS_MAX_SHARDS=20`, `OSS_EVICT_AFTER_MS=1500`), n2 joins empty, then two-document bulks go alternately to each node every 200 ms for 40 s, three runs each:
  - Fill: with hand-off on, n1 holds 13 and n2 7 at the end, in every run. Off: n1 20, n2 0.
  - No lost write: every bulk was acknowledged (0 errors) and the count equals the acknowledged documents in every hand-off run. One off run showed 694 of 696 acknowledged documents in a count taken without a refresh, which is the visibility lag of the last second, in the arm that does not hand off.
  - Write p99 over the 40 s: off 769 to 783 ms, hand-off 1,092 to 1,123 ms (+43%); p50 624 to 633 ms off, 638 to 644 ms on (+2%).
  - Store requests over the 40 s: GET and ranged GET 269 to 273 off, 395 to 397 on (+45%); PUT 356 to 364 off, 371 to 375 on (+4%); LIST and HEAD 0 in both.
  - Headroom 25% on top of hand-off: the same as hand-off alone (n1 13, n2 7; p99 1,107 to 1,126 ms; GETs 399 to 403). Hand-off does not use headroom, so this is no measure of headroom, whose own path (cap eviction when a shard is created past the cap) was not run.
  - One 120 s run each, in thirds of 40 s (p99 ms, GETs): off (776, 269), (693, 275), (702, 262); hand-off (1,134, 397), (701, 266), (676, 258). The extra cost is in the first third, while shards move; afterwards it equals off. About 125 more GETs for 7 shards moved, about 18 a shard.
  - Decision: the default stays off. The spec's switch rule asks for no p99 or request-count movement, and the move itself shows +43% p99 and +45% GETs over the window in which shards move. The steady state after it matches off. What would flip it: a rebalance whose p99 stays within 5% (the shards a hand-off opens are opened by the receiving node on its first write, which is where the GETs and the p99 are), or a measured gain for a fleet that is capped, which this run does not show because writes are not limited by the cap here. NOT-RUN: RustFS, a write rate that makes the full node the bottleneck, cap eviction with headroom, and more than two nodes.
