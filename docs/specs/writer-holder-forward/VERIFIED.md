# A write for a shard another live node holds goes to that node: verification

- AC-1: `a_shard_a_live_node_holds_is_not_opened_here_and_its_write_goes_to_that_node` (node, `tests/writer_opener.rs`). Seen failing against the old refusal (503 naming node-2).
- AC-2: `a_second_write_goes_to_the_holder_without_reading_the_head` (node, `tests/writer_opener.rs`): the opener asked once, one blob request (the index lookup) for the second write. Seen failing likewise. Emulator, provisional.
- AC-3: `a_holder_that_fails_is_asked_again_through_the_head` (node, `tests/writer_opener.rs`): the opener asked twice after a failed forward. Seen failing likewise.
- AC-4: `a_first_write_opens_the_shard_and_a_second_opens_nothing`, `groups_racing_for_an_unheld_shard_open_it_once` and `a_shard_with_a_history_is_refused_as_takeover` (node, `tests/writer_opener.rs`), unchanged.
