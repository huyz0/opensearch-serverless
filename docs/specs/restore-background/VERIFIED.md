# Verified: restore-background

- AC-1: `the_answer_comes_before_the_copy_and_the_pool_is_held_until_it_ends` (a gate holds the first ranged read; the answer is accepted while it is held; after the release the files are in both shards). The search over the restored index is not repeated here: the copy is the synchronous restore's, held by `a_restored_index_answers_a_search_with_what_the_snapshot_captured`.
- AC-2: `a_refusal_before_the_first_write_writes_nothing_and_starts_no_thread` (missing snapshot, uncaptured index, existing target: no put or copy, no thread). The stock path is not reached by the switch; no test asserts that.
- AC-3: `a_name_taken_between_the_check_and_the_create_is_400` (the create is held, the name is taken, the release gives 400 and the other index stands).
- AC-4: `the_answer_comes_before_the_copy_and_the_pool_is_held_until_it_ends` (marked while held, unmarked after, a retry is 400), `a_delete_of_the_marked_index_clears_it_and_the_name_restores_again`. The 503 of a search and a write on a marked index is held by the existing mark tests, not repeated.
- AC-5: `a_write_that_fails_after_the_answer_removes_every_index_it_made` (a failure at each of the five puts the thread makes), `a_read_that_fails_after_the_answer_removes_the_index`, and `a_restore_thread_that_panics_deletes_its_indices_and_frees_its_slot` (the guard, in a thread that panics).
- AC-6: `a_restore_past_the_cap_and_one_the_pool_cannot_hold_are_429`, `the_answer_comes_before_the_copy_and_the_pool_is_held_until_it_ends` (pool above its floor while held, back after), and the unit tests of the slot in `crates/node/src/restore_background.rs`. NOT-RUN: a thread that cannot be spawned.
- AC-7: `a_drain_cancels_the_copy_and_refuses_new_restores`, `a_drain_waits_for_a_copy_that_ends_in_time`, and the drain tests in `crates/node/src/restore_background.rs`. NOT-RUN: the drain through the drop of `DrainTimer`.
- AC-8: `a_background_restore_costs_what_the_synchronous_one_does` (`snapshot.restore_two.requests` unchanged). The counts are read through getters; there is no stats endpoint.
- AC-9: `a_restore_asked_to_wait_or_with_the_switch_off_is_synchronous`, `the_background_restore_is_off_with_two_and_thirty_seconds_until_set` (the three settings and their limits), and the existing restore tests, unchanged.

The slot's tests ran red against stubs first; the integration tests were written with the build and not seen red on a wrong result.
