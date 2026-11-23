# A node refuses writes the object store cannot take now

## Serves

R13.2: the Java shell's `WriteBackpressure`. A write is acknowledged only once its log record has landed; when the store throttles (`503 SlowDown`) every append retries with backoff, a fleet's writes stall for the client's whole timeout, and every append that finally fails fences its shard for a reopen. Waiting harder makes it worse. The node limits the writes it lets wait on the store at once, by how the store is answering, and refuses the rest at once with a 429 and a retry hint.

## Change

- `oss_writer::admission::WriteLimiter` (sans-IO: every call takes the time as nanoseconds): additive increase, multiplicative decrease on the writes in flight.
  - `admit(now)` takes a slot or refuses with `Throttled { retry_after_secs, reason }`; the slot is a `Permit`, given back on drop.
  - `on_append(now, elapsed, succeeded)` feeds how a log append went.
  - Refusal: more writes in flight than the limit, or, while the pressure (the larger of the smoothed append time and the oldest waiting write's age) exceeds the budget, more than the minimum in flight. `retry_after_secs` is the pressure in whole seconds plus one, between 1 and 30.
  - The limit is cut by a third (never below the minimum) when a write has waited longer than the target, or an append failed or its smoothed time passed the target; at most once per 500 ms. It grows by `limit * 1.02 + 0.5` while the smoothed time is under half the target, and by `4 / limit` between half and the target; never above the maximum. The smoothed time is `0.8 * old + 0.2 * sample`, the first sample taken whole.
- Settings (node scope): `serverless.write.backpressure.target_millis` 1,000, `.budget_millis` 5,000, `.min_in_flight` 4, `.max_in_flight` 1,024 (the Java's default grows with cores; the node sets it from them).
- The node's writer admits each shard group before applying anything and times the log commit it makes; a refused group fails each of its items with 429 `es_rejected_execution_exception` carrying the reason, with nothing applied and nothing logged.

No enabled switch: the limit starts at the maximum, so a healthy store never meets it; a deployment that wants none sets `max_in_flight` very high (principles: no downside, no switch).

## Acceptance criteria

- AC-1: writes up to the limit are admitted, the next is refused, and a dropped permit frees its slot.
- AC-2: a slow smoothed append, or a failed one, cuts the limit by a third and never below the minimum; two cuts within 500 ms are one.
- AC-3: fast appends grow the limit and it never passes the maximum; appends between half the target and the target grow it by the additive step.
- AC-4: while the pressure exceeds the budget a write is refused whenever more than the minimum are in flight, and admitted at the minimum; `retry_after_secs` is as above, and never over 30.
- AC-5: a write that has waited past the target cuts the limit at the next admit, though no append has ended (the store keeps appends in retries for tens of seconds).
- AC-6: a refused group is neither applied nor logged, and each item answers 429.
- AC-7: the settings are registered with the defaults above.

## Test plan

`crates/writer/src/admission/limiter.rs` unit tests with explicit times (AC-1 to AC-5); `crates/node/tests/roles/` over the writer role with a limiter at its minimum (AC-6); the node's registry test (AC-7).

## S3 request budget

None: the limiter reads no store and sends no request; it makes fewer appends under a throttling store, not more.

## Risks

- A limit cut by one slow minute climbs back geometrically (about 2% per fast append); the Java measured the additive-only climb at an hour and changed it, as here.
- The limiter is per node, not per shard: one slow shard's appends slow every shard's admission. The log is one per node, so the store sees them as one stream.
