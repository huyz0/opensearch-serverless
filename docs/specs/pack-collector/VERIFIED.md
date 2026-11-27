# Verified

Slice 1 only (ledger rotation and the sidecar). Emulator behind the in-process S3 front end, so request counts are exact; no latency numbers.

- AC-1: the existing node-pack suites (`crates/node/tests/roles/writer_packs.rs`, `crates/writer/tests/pack_writer.rs`) pass unchanged with collection off; `off_the_ledger_is_written_once` in crates/writer/tests/pack_collect.rs
- AC-2: `high_is_reserved_ahead_in_blocks` and `an_epoch_rotates_every_1024_ordinals` in crates/writer/tests/pack_collect.rs (the second parses the register with the Java-format `NodePackLedger`); `with_collection_on_a_first_publish_costs_the_same_and_reserves_ahead` in crates/node/tests/roles/writer_packs.rs
- AC-3: `a_sidecar_round_trips_at_144_bytes_an_epoch`, `marks_name_the_ordinals_still_to_read` and `full_epochs_are_dropped_but_the_newest` in crates/format/tests/pack_sidecar.rs; `the_sidecar_is_read_and_swapped_by_generation` and `at_1500_live_epochs_the_writer_stops` in crates/writer/tests/pack_collect.rs. The restart that re-reads only unmarked ordinals is the collector pass (slice 3).
- AC-10: `collection_is_off_by_default_and_read_with_packs_on` in crates/node/src/boot/config/pack_tests.rs; `the_settings_default_off_with_the_documented_sizes_and_bounds` in crates/node/src/writer/packs.rs. The collector's memory reservation is the collector pass (slice 3).

Not yet verified (later slices): AC-4 to AC-9.
