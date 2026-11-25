# Verified: takeover-budgets

In-process emulator with 20 ms held per call, so request and round counts are exact and nothing about time is claimed.

- AC-1: `taking_a_shard_its_writer_released_costs_what_the_table_says` in crates/node/tests/takeover_budgets.rs: 10 requests, no LIST (Java's take: 25). Seen red first: the rows were blocked and the gate refused.
- AC-2: `taking_a_shard_its_writer_released_costs_what_the_table_says` in crates/node/tests/takeover_budgets.rs: 10 round trips (Java: 24).
- AC-3: `taking_a_shard_its_writer_released_costs_what_the_table_says` in crates/node/tests/takeover_budgets.rs: the take is held and the next write is sequence number 2.

Not covered: RustFS and S3 request counts (a register swap from an unread generation is two requests there), a take from a dead writer (see proactive-takeover-faults), 1K-shard take time against the Java.
