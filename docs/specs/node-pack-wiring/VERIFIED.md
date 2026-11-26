# Verified

Emulator behind the in-process S3 front end, so request counts are exact. No latency or soak numbers: the R11 measurement runs after the collector exists, and the switch stays off.

- AC-1: `the_settings_default_off_with_the_documented_sizes_and_bounds` in crates/node/src/writer/packs.rs; `the_nodes_settings_form_a_valid_registry` in crates/node/src/settings.rs
- AC-2: `node_packs_are_off_by_default_and_on_with_the_documented_sizes` and `a_pack_size_outside_its_bounds_is_refused` in crates/node/src/boot/config/pack_tests.rs
- AC-3: `with_packs_off_a_publish_writes_blobs_and_no_pack_or_ledger` in crates/node/tests/roles/writer_packs.rs
- AC-4: `with_packs_on_a_publish_is_one_pack_put_and_names_the_pack` in crates/node/tests/roles/writer_packs.rs
- AC-5: `a_node_taking_over_a_packed_shard_reads_every_document` in crates/node/tests/roles/writer_packs.rs
- AC-6: `a_node_that_publishes_nothing_makes_no_pack_request` in crates/node/tests/roles/writer_packs.rs
- AC-7: `a_later_publish_reuses_the_writer_and_writes_the_ledger_once` in crates/node/tests/roles/writer_packs.rs
