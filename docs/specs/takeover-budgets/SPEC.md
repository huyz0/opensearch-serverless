# Taking a released shard costs what the table says

## Serves

Parity plan R8 exit gate: a take of a shard its writer released costs at most the Java's 25 requests over 24 round trips. Invariant I-1 (requests scale with nodes and bytes).

## Change

Two request-budget rows move from `blocked` to `enforced`: `take.released.requests` and `take.released.round_trips`. A test retires a node holding one written shard (publishing it and giving its head back), then measures a second node's first write to that shard through a round counter: the head read, the take, the descriptor and commit reads and the write's own log PUT. Nothing in the product changes. The test rig gains `cluster_rounds`, a cluster whose nodes reach the emulator through `Rounds`.

## Acceptance criteria

- AC-1: the first write by a node to a shard whose writer retired costs the requests in row `take.released.requests`, asserted by the meter, and no LIST.
- AC-2: the same write waits through the round trips in row `take.released.round_trips`.
- AC-3: every document written before the retire and by the second node is found after the take.

## Test plan

`crates/node/tests/takeover_budgets.rs`. Red first: the rows are blocked, so the gate refuses until they are enforced with the measured numbers. AC-3 would catch a take that cost less by skipping the commit restore.

## S3 request budget

Unchanged; this measures the take. The measurement includes the write's own log PUT, so it is an upper bound on the take alone.

## Switch

None; no behaviour changes.

## Risks

The emulator counts a register swap as one request; on S3 a swap from a generation the process did not just read is two. The row is the emulator count, as every row is (docs/request-budgets.md rules).
