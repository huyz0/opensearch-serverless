# The writer role's publish timer

## Serves

Parity plan R7.6, node assembly: [`WriterRole::publish_due`](../node-publish-loop/SPEC.md) is the pass; this is the thread that runs it. Ported from the shell's `BackgroundReconciler` schedule.

## Change

`oss-node`'s `writer` module gains `PublishTimer`: `PublishTimer::spawn(role, every)` starts one thread that runs `publish_due` every `every` until the timer is dropped, and dropping joins the thread.

## Acceptance criteria

- AC-1: a shard written after the timer started is published without the caller running a pass.
- AC-2: dropping the timer stops it promptly (well inside `every`) and no pass runs after the drop returns.
- AC-3: ticks over idle shards make no store request.

## Test plan

`crates/node/tests/writer_publish_timer.rs`: a role on the metered emulator with the real clock and age 0, ticks of 10 ms; bounded waits poll the meter.

## S3 request budget

The timer adds no request of its own: each tick is a pass, which publishes only the shards the lazy policy finds due ([node-publish-loop](../node-publish-loop/SPEC.md)). One thread a node, not a shard.

## Risks

- A panic in a pass ends the thread; the timer is then silent. The pass holds no unwinding state (locks recover from poisoning), so a respawn by the caller is safe.
