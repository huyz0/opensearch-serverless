# A writer's first holds, publishes and release cost what the table says

## Serves

Parity plan R8 exit gate: the writer's rows of the request-budget table stop being `blocked`. Invariant I-1 (requests scale with nodes and bytes).

## Change

Six rows move from `blocked` to `enforced`, each with the number the Rust writer measures on the emulator: `take.fresh.requests`, `publish.requests`, `release.requests`, `held_idle.index_requests_per_hour`, `held_idle.node_requests_per_hour` and `idle_released.node_requests_per_hour`. The three request rows are measured by `crates/node/tests/writer_budgets.rs` on a node over four shards; the three hourly rows by `crates/node/tests/idle_hour.rs`, which already runs an hour of the writer's passes. Nothing in the product changes. The Java numbers stay in the rows as references; they are not targets, and the rows say where the paths differ (the Java's idle hour includes lease renewal, which is `lease.idle.*` here). `search.held_idle.index_requests_target` stays `blocked` until the reader's periodic check exists.

## Acceptance criteria

- AC-1: a node's first holds of four never-written shards, its lease enrolment included, cost the requests in row `take.fresh.requests` and make no LIST.
- AC-2: one write to each of four held shards and then a publish of all four cost the requests in row `publish.requests` and make no LIST.
- AC-3: retiring a node that holds four published idle shards costs the requests in row `release.requests`, hibernates and gives back all four, and makes no LIST.
- AC-4: an hour of the writer's passes over four released shards makes no request (row `idle_released.node_requests_per_hour`), with no shard held after it; the held-shard hour is the same measure in rows `held_idle.index_requests_per_hour` and `held_idle.node_requests_per_hour`.

## Test plan

`crates/node/tests/writer_budgets.rs` and `crates/node/tests/idle_hour.rs`. Red first: the rows were blocked, so the gate refused; a budget one request too high fails as "improved" (seen for `take.fresh.requests`).

## S3 request budget

Unchanged; this pins what is measured.

## Switch

None.

## Risks

Emulator counts: a register swap is one request, on S3 two from a generation the process did not just read. The publish row is above the Java's 23 for a different path; it is recorded as measured, and a saving lowers it. The hourly rows count the requests of the passes the test calls, not of timers outside them (see idle-budgets).
