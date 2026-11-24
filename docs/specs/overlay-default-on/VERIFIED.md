# Verified

Emulator behind the S3 front end with the Java latency profile, release build, three nodes, one shard, 100 samples a series, one run each (`METER_OVERLAY=false|true cargo test -p oss-node --features "reader writer coordinator" --test freshness_meter --release -- --ignored --nocapture`). Provisional.

```
overlay off (commit push on, its default there): refresh=false ack p99=338.7 | visible p50=2005 p99=2236.6 max=2238.6 | misses 200 of 200 | per write Get 19.98 GetRange 111.0 Put 9.87 List 0.16 Delete 1.01
overlay on:                                      refresh=false ack p99=331.7 | visible p50=314.4 p99=368.4 max=390.4 | misses 186 of 200 | per write Get 3.23 Put 2.49 Delete 0.20
overlay on:                                      refresh=true  ack p99=403.5 | visible p50=317.8 p99=410.3 max=429.7 | misses 0 of 200   | per write Get 3.28 Put 3.29 List 0.06 Delete 0.17
```

Bar for the shell: 528.9 ms x 1.05 = 555.3 ms, met with the overlay on and missed (2.2 s) with it off, because with it off each refresh publishes a commit and every reader reopens it cold. The write's ack p99 is 2% lower with it on (within the 5%), the requests a write are a sixth of the GETs, a quarter of the PUTs and no range GETs, and nothing is worse.

- AC-1: `the_tail_overlay_is_on_and_visibility_everywhere_unless_set` in crates/node/src/boot/config/tests.rs
- AC-2: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-3: `after_the_takeover_nothing_acknowledged_is_lost_doubled_or_stale`, `the_overlay_is_rebuilt_against_the_successor_so_a_refreshed_write_is_never_missed`, `a_search_after_the_owner_died_never_counts_fewer_than_were_acknowledged_without_a_failure` in crates/node/tests/overlay_takeover.rs
- AC-4: NOT-RUN: the whole workspace was not run locally (disk); CI runs every suite.
