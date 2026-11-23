# Verified: write-backpressure

- AC-1: `writes_up_to_the_limit_are_admitted_and_the_next_is_refused` in crates/writer/src/admission/limiter.rs
- AC-2: `a_slow_smoothed_append_cuts_the_limit_by_a_third_and_two_cuts_in_500ms_are_one` in crates/writer/src/admission/limiter.rs, and `a_failed_log_append_lowers_the_limit_and_a_landed_one_does_not` in crates/node/tests/roles/writer_role.rs
- AC-3: `fast_appends_grow_the_limit_to_the_maximum_and_no_further` and `appends_between_half_the_target_and_the_target_grow_it_by_the_additive_step` in crates/writer/src/admission/limiter.rs
- AC-4: `writes_beyond_the_minimum_are_refused_while_appends_exceed_the_budget` and `the_retry_hint_never_passes_thirty_seconds` in crates/writer/src/admission/limiter.rs
- AC-5: `a_write_waiting_past_the_target_cuts_the_limit_though_no_append_ended` in crates/writer/src/admission/limiter.rs
- AC-6: `a_group_the_limiter_refuses_is_neither_applied_nor_logged_and_answers_429` in crates/node/tests/roles/writer_role.rs
- Boot variables: `the_write_limit_is_read_from_the_environment_and_bounded` in crates/node/tests/server/boot.rs
- AC-7: `the_nodes_settings_form_a_valid_registry` in crates/node/src/settings.rs

NOT-RUN: coverage (no cargo-llvm-cov in the sandbox); a run against a store that really throttles (the emulator has no SlowDown); the `Retry-After` header on a single-document write (the 429's reason carries the hint; the header is for the document routes' own piece).
