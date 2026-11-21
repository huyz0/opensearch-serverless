# Verified: node-freshness-meter

Numbers are `provisional`: emulator behind the S3 front end, Java latency profile (divisor 1), one run, 100 samples a series after 5 warm-up writes, three nodes, one shard, explicit mapping. Run: `cargo test -p oss-node --features "reader writer coordinator" --test freshness_meter -- --ignored --nocapture`.

```
FRESHMETER|refresh=false|ack p50=304.6 p99=405.9 ms|visible p50=326.3 p99=435.2 max=438.6 ms (n=100)|misses 151 of 200|per write: Get 2.780 Put 2.590
FRESHMETER|refresh=true|ack p50=456.4 p99=507.8 ms|visible p50=473.3 p99=544.2 max=575.1 ms (n=100)|misses 0 of 200|per write: Get 3.850 Put 4.660 List 0.050
```

Run again on main after the refresh wait, the lazy publish leaving the shard lock, and the unanswered-writer fix, once for each `METER_TAIL` visibility (one run each, 100 samples a series). Bar: 528.9 ms x 1.05 = 555.3 ms.

```
visibility=owner      refresh=false visible p50=316.1 p99=400.4 | refresh=true ack p99=488.7 visible p50=453.2 p99=497.3 | misses 200 of 200 | per write Get 3.67 Put 4.58 List 0.05
visibility=serving    refresh=false visible p50=318.5 p99=394.7 | refresh=true ack p99=411.9 visible p50=336.6 p99=427.2 | misses 0 of 200   | per write Get 2.79 Put 3.37 List 0.05
visibility=everywhere refresh=false visible p50=323.7 p99=357.2 | refresh=true ack p99=380.7 visible p50=344.4 p99=396.3 | misses 0 of 200   | per write Get 2.80 Put 3.36 List 0.05
```

What this shows (provisional, emulator): under `serving` and `everywhere` a `refresh=true` write is visible in about 337-344 ms p50 and under 430 ms p99, never missed, for 1.2 PUTs and 0.9 GETs a write fewer than `owner`, which publishes a commit on every refresh and is missed by every first search. `refresh=false` costs the same in all three. The price is the grant ask of `everywhere`: a search that finds no grant (the first in each 500 ms window on a shard) pays one round trip to the writer, about +7 ms p50 on `refresh=false` visibility against `owner`. Two defects the earlier run showed are fixed: the lazy publish held the shard lock through its uploads (refresh=true p99 705-766 ms), and a reader whose lease view lost the writer for one ask refused the search on the next (0 to 2 misses of 200).

- AC-1: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-2: `the_percentile_is_the_nearest_rank` in crates/node/tests/freshness_meter.rs
- AC-3: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-4: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
- AC-5: `a_write_is_visible_on_the_other_nodes_within_the_shells_bar` in crates/node/tests/freshness_meter.rs
