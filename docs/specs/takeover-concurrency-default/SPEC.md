# A takeover pass takes 16 shards at once for 60 seconds

## Serves

Parity plan R8 exit gate: takeover of 1,000 shards within 10% of the Java's time. ADR 0021 (proactive takeover); the measurements are in [takeover-scale](../takeover-scale/SPEC.md). Principle: a default moves only on a measurement that shows no downside.

## Change

`serverless.takeover.proactive.max_concurrent` (env `OSS_PROACTIVE_MAX_CONCURRENT`) defaults to 16, from 4, and `serverless.takeover.proactive.pass_millis` (`OSS_PROACTIVE_PASS_MS`) to 60,000, from 30,000. Both stay settings with their bounds, so `OSS_PROACTIVE_MAX_CONCURRENT=4` and `OSS_PROACTIVE_PASS_MS=30000` are the old behaviour; the suites that pin four at once keep running with it. Nothing else changes; a pass makes the same requests.

## Acceptance criteria

- AC-1: the node's configuration defaults to 16 at once and 60,000 ms, reads both from the environment and refuses values out of bounds.
- AC-2: a pass over a log naming 938 shards of one dead node, at the Java soak's mean request latency, takes all of them in one pass within 1.10 times the Java's 57.0 s.
- AC-3: a live node's write p99 while a 16-wide pass runs is within 5% of its p99 with no pass.

## Test plan

AC-1 in `crates/node/src/boot/config/tests.rs`; AC-2 and AC-3 by hand in `crates/node/tests/takeover_scale.rs` (ignored, emulator).

## S3 request budget

Unchanged: a take costs the same requests; they are sent sooner.

## Switch

The two settings above. Default on at 16 and 60 s because the measurements show a 1,000-shard takeover at the Java's time (4 wide: 2.3 times) and no write p99 cost at 16.

## Risks

Each take reserves its engine's worst case from the node's pool and a pass defers a shard when there is no room (ADR 0021), but no test forces that deferral at any width, and the memory outside the engine reservation (log reads, restore buffers) is four times more in flight at once. A node with a small pool may defer more of a pass. The 60 s limit is tuned to the emulator's mean latency: 55.5 s measured, so at a real network's tail a pass defers its remainder to the retry (staggered, backed off) or to the next write. The measurements are on the emulator (provisional) and the write latency is measured on the node that runs the pass, to its own shard.
