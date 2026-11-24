# A refused single-document write tells the client how long to wait

## Serves

R13.2: the rest of [write-backpressure](../write-backpressure/SPEC.md). The limiter computes `retry_after_secs` for every refusal, but a single-document write's 429 carried it only inside the message text; a client acting on the header had none.

## Change

- `WriteLimiter::retry_after(now)`: the hint a refusal at `now` would give (the pressure in whole seconds plus one, 1 to 30), asked without admitting or refusing.
- `WriterRole::retry_after_secs()`: that hint at the role's clock, none without a limiter.
- The single-document write routes add `Retry-After: <seconds>` to an answer whose status is 429. A bulk answers 200 with per-item 429s, so it carries no header.

Not changed: the 429 body, the limiter's decision, the settings. The hint is read after the refusal, so it can differ by the time between the two; it is a hint.

## Acceptance criteria

- AC-1: a single-document write the limiter refuses answers 429 with a `Retry-After` of whole seconds between 1 and 30, and the same write once admitted answers 201 with no such header.
- AC-2: `retry_after` is 1 under no pressure, grows with the oldest waiting write, never passes 30, and neither admits nor refuses.

## Test plan

`a_document_the_limiter_refuses_answers_429_with_a_retry_after` in crates/node/tests/server/doc_route.rs (AC-1); `the_hint_a_refusal_would_give_is_asked_without_refusing` in crates/writer/src/admission/limiter.rs (AC-2).

## S3 request budget

None: the limiter reads no store.

## Switch

None: a header on an answer that is already a refusal; no downside, no switch.

## Risks

A client that retries at exactly the hint from many connections arrives together; the hint is a floor, and the limiter refuses again if pressure persists.
