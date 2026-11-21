# Verified: node-freshness-meter

Numbers are `provisional`: emulator behind the S3 front end, Java latency profile (divisor 1), one run, 100 samples a series after 5 warm-up writes, three nodes, one shard, explicit mapping. Run: `cargo test -p oss-node --features "reader writer coordinator" --test freshness_meter -- --ignored --nocapture`.

```
FRESHMETER|refresh=false|ack p50=304.6 p99=405.9 ms|visible p50=326.3 p99=435.2 max=438.6 ms (n=100)|misses 151 of 200|per write: Get 2.780 Put 2.590
FRESHMETER|refresh=true|ack p50=456.4 p99=507.8 ms|visible p50=473.3 p99=544.2 max=575.1 ms (n=100)|misses 0 of 200|per write: Get 3.850 Put 4.660 List 0.050
```

Bar: 528.9 ms x 1.05 = 555.3 ms. `refresh=true` passes by 11 ms, so it is close and one run. `refresh=false` misses on the first search after the ack in 151 of 200 cases: the push is asynchronous, so visibility trails the ack by tens of ms (visible p50 21 ms after ack p50). `refresh=true` costs about 2 PUTs a write more than `refresh=false` (a publish), which the shell avoids by waiting for the readers' push; the overlay wait is the other thread's piece.

- AC-1: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-2: `the_percentile_is_the_nearest_rank` in crates/node/tests/freshness_meter.rs
- AC-3: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-4: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-5: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
