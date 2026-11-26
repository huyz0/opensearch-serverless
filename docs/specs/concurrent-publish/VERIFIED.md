# Verified

Emulator behind the in-process S3 front end, so request counts are exact. No latency numbers: the R11 measurement runs once the collector exists.

- AC-1: the existing publish suites (`crates/node/tests/roles/writer_publish.rs`, `writer_staging.rs`, `writer_logged.rs`) pass unchanged with packs off; `one_thread_runs_serially_on_the_caller_and_none_for_nothing` in crates/node/src/writer/bounded.rs
- AC-2: `two_due_shards_of_one_pass_share_one_pack` in crates/node/tests/roles/writer_packs.rs; `different_shards_overlap_only_with_per_shard_gates` in crates/node/src/writer/gate.rs
- AC-3: `no_more_than_the_given_threads_run_at_once_and_results_keep_their_order` in crates/node/src/writer/bounded.rs; `with_one_thread_each_shard_is_a_pack_of_its_own` in crates/node/tests/roles/writer_packs.rs
- AC-4: `a_shard_never_has_two_publishes_in_flight` in crates/node/src/writer/gate.rs
- AC-5: `a_failing_shard_is_reported_and_the_others_are_published` in crates/node/tests/roles/writer_packs.rs
- AC-6: `a_failing_shard_is_reported_and_the_others_are_published` in crates/node/tests/roles/writer_packs.rs (the counts and the failure text are those of the serial pass)
- AC-7: `the_settings_default_off_with_the_documented_sizes_and_bounds` in crates/node/src/writer/packs.rs; `a_pack_size_outside_its_bounds_is_refused` in crates/node/src/boot/config/pack_tests.rs
