# Verified: node-lease-refresh

- AC-1: `the_lease_refresh_interval_is_set_apart_from_the_renewal` and `a_file_store_node_takes_the_defaults` in crates/node/src/boot/config.rs
- AC-2: `what_cannot_be_used_is_refused_by_name` in crates/node/src/boot/config.rs
- AC-3: `with_equal_periods_each_tick_renews_and_reads`, `a_faster_read_ticks_at_its_period_and_renews_on_its_own` and `a_slower_read_ticks_at_the_renewal_and_reads_on_its_own` in crates/node/src/boot/renew.rs
- AC-4: `a_read_that_failed_is_due_again_at_the_next_tick` and `a_renewal_is_due_once_a_period_even_when_it_failed` in crates/node/src/boot/renew.rs
- AC-5: `a_file_store_node_takes_the_defaults` in crates/node/src/boot/config.rs, and the meter lines below at the default
- AC-6: `an_announce_round_trips` and `a_truncated_or_empty_announce_is_refused` in crates/transport/src/actions/lease.rs
- AC-7: `an_unknown_joiner_makes_the_node_read_the_fleet_once`, `a_joiner_already_held_reads_nothing` and `a_restarted_joiner_is_not_the_one_held` in crates/node/src/lease_announce.rs
- AC-8: `announces_inside_the_gap_cost_one_read_now_and_one_when_it_ends` and `a_read_that_failed_is_not_reported` in crates/node/src/lease_announce.rs
- AC-9: `a_joiner_is_seen_by_every_node_inside_the_read_period` in crates/node/tests/lease_announce.rs (the announce thread sends to every live peer; the sender caps at `MAX_ANNOUNCE` and bounds each call by `SEND_TIMEOUT`)
- AC-10: `an_unsigned_request_never_reaches_a_handler` in crates/node/tests/handlers.rs (every action the registry holds, the announce included, sits behind the signature check)
- AC-11: `a_joiner_is_seen_by_every_node_inside_the_read_period` in crates/node/tests/lease_announce.rs, and the meter lines below

Measured on the emulator (provisional), three nodes, lease lifetime 30 s, S3 latency profile of the Java shell, idle 40 s then three joins, `LEASE_POLL_MS` unset / 2000 / 1000 (main before the lease re-read fix of #263, which adds about 180 GETs an hour at the default):

```
LEASEMETER|refresh=default  |idle per node per hour: Get  720 Put 360|join seen by all after (ms): [10444, 10467, 10662]
LEASEMETER|refresh=2000 ms  |idle per node per hour: Get 2130 Put 450|join seen by all after (ms): [ 2407,  2440,  2452]
LEASEMETER|refresh=1000 ms  |idle per node per hour: Get 3840 Put 390|join seen by all after (ms): [ 1365,  1382,  1435]
```

(Put 360 is the renewal every 10 s; the 450 and 390 are the same renewals plus the digest the nodes write now and then.) The freshness meter (`METER_REFRESH_MS`, lease lifetime 3 s so the default is 1,000 ms and 5x is 200 ms), `everywhere`, 100 samples:

```
default 1000 ms  refresh=true visible p50=331.5 p99=408.1 ms, misses 0 of 200   refresh=false visible p50=315.9 p99=395.5 ms
5x       200 ms  refresh=true visible p50=332.7 p99=405.4 ms, misses 0 of 200   refresh=false visible p50=316.6 p99=382.7 ms
```

Reading leases five times as often does not change how long a write takes to be visible to a reader (p50 and p99 within noise, 0 misses both): the tail overlay pushes the write; the lease view decides who is live, not what is fresh. It shortens how long a new node goes unseen by the others from about 10.5 s to 2.4 s (at 2 s) and 1.4 s (at 1 s), for 2,130 and 3,840 idle GETs a node-hour against 720.

Joiner announces itself (AC-11), the same meter at the default period, idle 100 s, with the announce and with the announce thread's send disabled (provisional, emulator):

```
LEASEMETER|announce on |idle per node per hour: Get 912 Put 456|join seen by all after (ms): [ 775, 1054, 1043]
LEASEMETER|announce off|idle per node per hour: Get 900 Put 444|join seen by all after (ms): [2145, 10260, 20433]
```

A join is seen in about a second instead of up to a period (the 2.1 s of the first join with it off is a read that happened to fall near it), and idle requests are within 1.5% (a join costs each peer one read; nothing is added while no node joins). A 40 s idle window read 930/480 against 750/390 with it off: that was the window catching a different number of 10 s ticks, which the 100 s window settles.
