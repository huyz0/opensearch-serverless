# Verified: forwarding a bulk to a shard owner

- AC-1: `a_signed_bulk_reaches_the_owner_and_its_answer_comes_back` in crates/node/tests/forward.rs
- AC-2: `a_refusal_with_early_revoke_off_reads_nothing` in crates/node/tests/forward.rs
- AC-3: `a_confirmed_refusal_revokes_the_lease_and_answers_gone` in crates/node/tests/forward.rs
- AC-4: `a_refusal_early_revoke_declines_is_a_failed_call` in crates/node/tests/forward.rs
- AC-5: `other_failures_never_reach_early_revoke` and `a_timeout_never_reaches_early_revoke` in crates/node/tests/forward.rs
- AC-6: `a_bulk_too_large_to_send_is_refused_without_a_call` in crates/node/tests/forward.rs
- AC-6: also `every_error_says_what_happened` in crates/node/tests/forward.rs
