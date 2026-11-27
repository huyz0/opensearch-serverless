# Painless compile cache: verified

Run on 2026-11-27 (Linux cloud container, debug build, emulator store, 2 cores; provisional). The cache tests and the node pool test were written before the module and failed to compile; the `encode` test was written after the wiring and was not seen red.

- AC-1: `a_repeated_key_is_computed_once` and `a_cached_blob_is_the_blob_and_no_input_is_missed` (crates/reader/src/painless_cache_tests.rs).
- AC-2: `another_key_gets_its_own_value` and `a_cached_blob_is_the_blob_and_no_input_is_missed` (a changed param, field kind and context each give their own blob).
- AC-3: `the_oldest_entry_leaves_at_the_entry_bound` and `charged_bytes_never_pass_the_budget_and_a_value_over_it_is_not_kept`.
- AC-4: `the_cache_is_paid_for_by_the_pool_and_only_with_the_switch_on` (crates/node/tests/painless_cache_pool.rs: 4,194,304 bytes held from the query-execution pool, a 1 MiB pool leaves the cache off and the search is 200) and `removing_the_cache_drops_what_paid_for_it`.
- AC-5: `an_error_is_not_cached` and, through encode, `a_cached_blob_is_the_blob_and_no_input_is_missed`.
- AC-6: `the_cache_is_paid_for_by_the_pool_and_only_with_the_switch_on`: with the switch off the pool reads 0 in use.
- AC-7: `cargo test -p oss-node --features coordinator,extensions --test painless_default_cost -- --ignored --nocapture`, three rounds of 2,000 searches, p50 (us): weight 613 to 656, expression 864 to 900, Painless 1,729 to 1,797, against 2,733 to 2,797 before the cache. The Painless script_score is 2.7 to 2.9 times a weight at p50 (4.2 to 4.4 before), 2.0 times an expression; p99 3,075 to 3,708 against 5,178 to 6,344. A new source's first request 3,417 us, its repeats p50 1,840 us. The remainder is the engine's own parse, specialise and lowering per segment, not cached here. A release run is NOT-RUN.
