# Verified: the builder fetches what a writer acknowledged when a shard opens

- AC-1: `the_first_ask_fetches_the_writer_and_builds_over_its_answer` in crates/node/tests/overlay_fetch.rs
- AC-2: `a_complete_answer_grants_and_an_incomplete_one_does_not` in crates/node/tests/overlay_fetch.rs
- AC-3: `further_asks_fetch_nothing_and_a_new_stage_fetches_again` in crates/node/tests/overlay_fetch.rs
- AC-4: `a_stage_without_a_writer_asks_nobody` in crates/node/tests/overlay_fetch.rs and `an_unanswered_fetch_holds_nothing_is_not_repeated_and_pushes_still_build` in crates/node/tests/overlay_fetch.rs
- AC-5: `a_stage_gives_the_writer_that_staged_it` in crates/reader/tests/commit_push.rs
- AC-1 (transport): `the_builders_fetcher_trait_reaches_the_same_writer` in crates/node/tests/tail_fetch.rs
- AC-6: `a_fetch_queued_for_a_stage_the_shard_left_asks_nobody` in crates/node/tests/overlay_fetch.rs and `a_fetch_queued_for_a_closed_shard_asks_nobody` in crates/node/tests/overlay_fetch.rs
