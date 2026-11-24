# Verified

Emulator behind the S3 front end, so numbers are provisional.

- AC-1: `a_commit_the_owner_publishes_reaches_every_node_that_has_the_shard_open` in crates/node/tests/commit_push_wired.rs
- AC-2: `a_commit_the_owner_publishes_reaches_every_node_that_has_the_shard_open` in the same file
- AC-3: `with_the_setting_off_an_opened_shard_stays_on_its_commit` in the same file
- AC-4: `a_commit_is_taken_locally_at_once_and_sent_by_the_worker` in crates/node/src/commit_push/tests.rs
- AC-5: `a_full_queue_drops_the_commit_and_the_publisher_never_waits`, `after_stop_nothing_is_queued` in crates/node/src/commit_push/tests.rs
- AC-6: `the_commit_push_is_on_unless_the_variable_turns_it_off` in crates/node/src/boot/config/tests.rs
- AC-7: `the_owner_counts_its_own_refreshed_writes_from_its_first_search` in crates/node/tests/overlay_takeover.rs
- AC-8: `a_commit_its_own_writer_published_is_kept_for_the_open_that_follows` in crates/node/tests/server/push_read.rs; `after_the_takeover_nothing_acknowledged_is_lost_doubled_or_stale` in crates/node/tests/overlay_takeover.rs
- AC-10: `the_overlay_is_rebuilt_against_the_successor_so_a_refreshed_write_is_never_missed` in crates/node/tests/overlay_takeover.rs (a survivor refused as behind answers within 10 s; the release itself has no test of its own: before it, one run in about eight left a survivor refusing for 30 s, after it a persistent refusal was not seen in the 7 further full runs that completed)
- AC-9: `a_search_after_the_owner_died_never_counts_fewer_than_were_acknowledged_without_a_failure`, `after_the_takeover_nothing_acknowledged_is_lost_doubled_or_stale`, `the_overlay_is_rebuilt_against_the_successor_so_a_refreshed_write_is_never_missed` in crates/node/tests/overlay_takeover.rs

Before the fix, with the overlay off, a write made after the first search was invisible on all three nodes (the owner's included) 36 s later; with the overlay on, the owner's first search after a refreshed write counted 0 of 1, and after a takeover the new owner answered 503 `shard_behind_exception` to every search for 30 s.

Freshness meter (`METER_PUSH`, release build, overlay on, `everywhere`, one run each, bar 555.3 ms), provisional:

```
push sent to readers: refresh=true visible p50=330.6 p99=941.6 | per write Get 4.74 GetRange 44.54 Put 3.85   (bar missed)
push off, local take on: refresh=true visible p50=318.6 p99=403.3 | per write Get 2.99 Put 3.27
push off:             refresh=true visible p50=317.2 p99=418.4 | per write Get 3.25 Put 3.27
```

The local take costs nothing measurable; the send to other readers does with the overlay on, so it defaults off there.

Final settings (overlay on, push send off by default, takeover push on): refresh=true visible p50=318.8 p99=408.7 ms (n=100), misses 0 of 200, per write Get 3.29 Put 3.30 List 0.06; refresh=false visible p99=368.6.
