# Verified: takeover-budgets

In-process emulator with 20 ms held per call, so request and round counts are exact and nothing about time is claimed.

- AC-1: `taking_a_shard_its_writer_released_costs_what_the_table_says` in crates/node/tests/takeover_budgets.rs: 10 requests, no LIST (Java's take: 25). Seen red first: the rows were blocked and the gate refused.
- AC-2: `taking_a_shard_its_writer_released_costs_what_the_table_says` in crates/node/tests/takeover_budgets.rs: 10 round trips (Java: 24).
- AC-3: `taking_a_shard_its_writer_released_costs_what_the_table_says` in crates/node/tests/takeover_budgets.rs: the take is held and the next write is sequence number 2.
- AC-4: `taking_a_shard_its_writer_released_costs_the_same_on_rustfs` in crates/node/tests/takeover_budgets.rs, run by hand with `RUSTFS_BIN` (RustFS 1.0.6, zip SHA-256 as in the run recipe): 10 requests, 10 round trips, no LIST, the same ten calls as the emulator's (limits 25 and 24). Provisional: local process, no network.

Not covered: real S3 request counts (a register swap from an unread generation is two requests there), a take from a dead writer (see proactive-takeover-faults), 1K-shard take time against the Java.
