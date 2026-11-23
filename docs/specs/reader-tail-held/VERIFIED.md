# Verified: the reader holds the writes its writer pushed

- AC-1: `an_untracked_shard_is_unwanted_and_a_tracked_one_keeps_what_is_above_its_checkpoint` in crates/node/tests/tail/tail_held.rs
- AC-2: `a_full_shard_refuses_until_an_engine_opens_on_a_newer_commit` in crates/node/tests/tail/tail_held.rs
- AC-3: `a_shard_is_kept_thirty_seconds_after_its_last_engine_closes` in crates/node/tests/tail/tail_held.rs
- AC-3 (expired and refusing): `an_expired_shard_that_was_refusing_is_still_let_go` in crates/node/tests/tail/tail_held.rs
- AC-4: `from_gives_what_is_held_at_or_after_an_ordinal_in_order` in crates/node/tests/tail/tail_held.rs
- AC-5: `an_answer_sets_a_renews_or_clears_the_grant` in crates/node/tests/tail/tail_held.rs
- AC-5 (refused records): `an_answer_whose_records_were_refused_is_not_complete` in crates/node/tests/tail/tail_held.rs
- AC-6: `a_grant_needs_every_condition` in crates/node/tests/tail/tail_held.rs
- AC-7: `handle_answers_the_shards_whose_records_were_not_taken` in crates/node/tests/tail/tail_held.rs
