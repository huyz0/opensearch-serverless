# Verified: the writer answers a reader's newest-commit ask

- AC-1: `the_handler_answers_the_staged_manifest` in crates/node/tests/writer_newest.rs
- AC-2: `a_shard_never_published_not_held_or_released_answers_none` in crates/node/tests/writer_newest.rs
- AC-3: `a_later_stage_replaces_and_reopening_drops` in crates/node/tests/writer_newest.rs
- AC-4: `an_oversized_manifest_and_an_unheld_shard_are_refused` in crates/node/tests/writer_newest.rs
- AC-5: `a_malformed_ask_is_refused_and_no_request_is_made` in crates/node/tests/writer_newest.rs
- AC-6: `a_manifest_staged_while_the_shard_is_released_is_not_kept` in crates/node/tests/writer_newest.rs
