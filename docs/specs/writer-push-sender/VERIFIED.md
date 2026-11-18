# Writer push sender: verification

- AC-1: `a_published_commit_goes_once_to_each_target_as_published` (node, `src/push_sender.rs`).
- AC-2: `a_target_that_fails_does_not_stop_the_others` (node, `src/push_sender.rs`).
- AC-3: `with_no_targets_or_pushing_off_nothing_is_sent` (node, `src/push_sender.rs`).
- AC-4: `readers_that_answered_open_are_remembered_until_one_says_not` and `a_shard_with_no_open_reader_is_not_remembered` (node, `src/push_sender.rs`).
- AC-5: `the_remembered_shards_stay_within_the_bound` (node, `src/push_sender.rs`).
