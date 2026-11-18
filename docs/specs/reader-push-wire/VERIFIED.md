# Reader push wire: verification

- AC-1: `every_recorded_push_message_is_the_javas_both_ways` (transport, `tests/wire/push.rs`, push_request rows).
- AC-2: `every_recorded_push_message_is_the_javas_both_ways` (transport, `tests/wire/push.rs`, push_response, newest_request and newest_response rows).
- AC-3: `a_commit_of_nothing_and_no_commit_are_told_apart` (transport, `tests/wire/push.rs`).
- AC-4: `a_push_claiming_more_blocks_than_it_holds_is_refused_before_it_allocates` (transport, `tests/wire/push.rs`).
- AC-5: `the_action_names_are_the_shells` (transport, `tests/wire/push.rs`).
