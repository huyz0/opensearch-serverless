# Verified

Emulator behind the S3 front end with the Java latency profile, release build, three nodes, one shard, 100 samples a series, one run each (`METER_OVERLAY=false|true cargo test -p oss-node --features "reader writer coordinator" --test freshness_meter --release -- --ignored --nocapture`). Provisional.

Overlay on, 300 samples a series, one run (search latency is the time of each search that counted the document; one per node per sample):

```
refresh=false: ack p50=302.9 p99=336.3 | visible p50=315.7 p99=380.6 max=405.1 | search p50=2.6 p99=32.3 ms | per write Get 3.22 Put 2.49 List 0.02
refresh=true:  ack p50=329.7 p99=400.3 | visible p50=341.5 p99=412.4 max=422.7 | search p50=4.7 p99=31.4 ms | per write Get 3.47 Put 3.36 List 0.03 | misses 0 of 600
```

Overlay off (commit push on, its default there), 100 samples a series, one run:

```
refresh=false: ack p50=306.3 p99=342.7 | visible p50=2015.9 p99=2254.1 max=2286.7 | search p50=166.6 p99=279.6 ms | per write Get 20.05 GetRange 111.0 Put 9.90 List 0.16 Delete 1.00 | misses 200 of 200
refresh=true:  ack p50=450.6 p99=499.9 | visible p50=1379.0 p99=1779.1 max=1800.4 | search p50=456.4 p99=646.2 ms | per write Get 14.48 GetRange 321.0 Put 7.71 List 0.10 Delete 0.98 | misses 200 of 200
```

Bar for the shell: 528.9 ms x 1.05 = 555.3 ms: met with the overlay on, missed by 3 to 4 times with it off, because each refresh publishes a commit and every reader reopens it cold (the search p50 of 167 to 456 ms is that reopen). With it on the write's ack p99 is 1.9% lower (refresh=false) and 20% lower (refresh=true) than off, requests a write are a sixth of the GETs, a quarter of the PUTs and no range GETs, and searches are 60 to 100 times faster at p50. One run each: a p99 over 100 to 300 samples is close to a maximum, so the differences under a few percent are noise; the others are far outside it.

Not measured, and the reason the ADR names them: memory and timers across many shards (the overlay builder charges the `open_indexes` pool without evicting, the tail keep and push queues are bounded per shard, the flush timer is per node), the rate and length of `shard_behind_exception` refusals in a long run, and the `serving` and `owner` visibilities with the overlay on.

- AC-1: `the_tail_overlay_is_on_and_visibility_everywhere_unless_set` in crates/node/src/boot/config/tests.rs
- AC-2: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-3: `after_the_takeover_nothing_acknowledged_is_lost_doubled_or_stale`, `the_overlay_is_rebuilt_against_the_successor_so_a_refreshed_write_is_never_missed`, `a_search_after_the_owner_died_never_counts_fewer_than_were_acknowledged_without_a_failure` in crates/node/tests/overlay_takeover.rs
- AC-4: NOT-RUN: the whole workspace was not run locally (disk); CI runs every suite.
