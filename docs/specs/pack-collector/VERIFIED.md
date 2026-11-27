# Verified

Slices 1 and 2 (ledger rotation and the sidecar; the index registers), and the election part of slice 3. Emulator behind the in-process S3 front end, so request counts are exact; no latency numbers.

- AC-1: the existing node-pack suites (`crates/node/tests/roles/writer_packs.rs`, `crates/writer/tests/pack_writer.rs`) pass unchanged with collection off; `off_the_ledger_is_written_once` in crates/writer/tests/pack_collect.rs
- AC-2: `high_is_reserved_ahead_in_blocks` and `an_epoch_rotates_every_1024_ordinals` in crates/writer/tests/pack_collect.rs (the second parses the register with the Java-format `NodePackLedger`); `with_collection_on_a_first_publish_costs_the_same_and_reserves_ahead` in crates/node/tests/roles/writer_packs.rs
- AC-3: `a_sidecar_round_trips_at_144_bytes_an_epoch`, `marks_name_the_ordinals_still_to_read` and `full_epochs_are_dropped_but_the_newest` in crates/format/tests/pack_sidecar.rs; `the_sidecar_is_read_and_swapped_by_generation` and `at_1500_live_epochs_the_writer_stops` in crates/writer/tests/pack_collect.rs. The restart that re-reads only unmarked ordinals is the collector pass (slice 3).
- AC-10: `collection_is_off_by_default_and_read_with_packs_on` in crates/node/src/boot/config/pack_tests.rs; `the_settings_default_off_with_the_documented_sizes_and_bounds` in crates/node/src/writer/packs.rs. The collector's memory reservation is the collector pass (slice 3).
- AC-4: `crates/format/tests/pack_index.rs`; in crates/writer/tests/pack_index.rs an add always moves the generation, a stale removal loses, the retry stops at 16 attempts and 24 contending adds all land; in crates/writer/tests/pack_collect.rs the node is indexed before its first pack and again at a rotation, and no pack is written when the add fails; `with_collection_on_a_first_publish_indexes_the_node_and_reserves_ahead` in crates/node/tests/roles/writer_packs.rs
- AC-5 (part): `members_that_agree_pick_the_same_one_collector`, `an_empty_membership_has_no_collector_and_one_member_is_always_it` and the pinned choice in crates/writer/tests/pack_election.rs. Two disagreeing nodes both running safely needs the pass itself.

Not yet verified (later slices): the rest of AC-5, AC-6 to AC-9.
