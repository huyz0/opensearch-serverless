# Verified: idle-budgets

In-process emulator; the passes are called, with the clock the test gives them, 14,400 publish and hibernate passes and 60 janitor passes for the hour. Counts are exact; nothing about time is claimed.

- AC-1: `an_hour_of_passes_over_idle_held_shards_asks_for_nothing` in crates/node/tests/idle_hour.rs: 0 requests (gate: at most 16 a shard-hour; Java: 64 an hour for four shards). Seen red first: the row was blocked.
- AC-2: `an_hour_of_passes_over_hibernated_shards_asks_for_nothing` in crates/node/tests/idle_hour.rs: 0 requests. Seen red first: the row was absent.
- AC-3: `an_hour_of_passes_over_idle_held_shards_asks_for_nothing` and `an_hour_of_passes_over_hibernated_shards_asks_for_nothing` in crates/node/tests/idle_hour.rs assert the four shards are still held or asleep after the hour.

Not covered: the node's own lease renewal (rows `lease.idle.*`), the orphan sweep and reclaim drain (off or empty), the timers' own threads, and a real clock. The Java's `held_idle.index_requests_per_hour` and `held_idle.node_requests_per_hour` rows stay blocked: they pin the Java's numbers, not a target.
