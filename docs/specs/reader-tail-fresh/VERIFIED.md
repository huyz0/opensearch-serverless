# Verified: reader-tail-fresh

- AC-1: `without_grants_every_search_may_answer_and_nothing_is_asked` in crates/node/tests/tail_fresh.rs
- AC-2: `under_a_grant_a_search_answers_without_asking` in crates/node/tests/tail_fresh.rs
- AC-3: `without_a_grant_the_writer_is_asked_to_renew_or_to_subscribe_anew` in crates/node/tests/tail_fresh.rs
- AC-4: `a_shard_with_no_engine_only_asks_how_far_refreshing_writes_went` in crates/node/tests/tail_fresh.rs
- AC-5: `a_writer_that_does_not_answer_is_treated_as_dead` and `a_writer_that_did_not_answer_is_not_asked_again_for_the_same_search` in crates/node/tests/tail_fresh.rs
- AC-6: `an_answer_without_a_grant_leaves_the_commit_to_be_compared_with_the_claim` in crates/node/tests/tail_fresh.rs
- AC-7: `what_to_ask_follows_the_engine_and_the_subscription_held` in crates/node/tests/tail_fresh.rs
