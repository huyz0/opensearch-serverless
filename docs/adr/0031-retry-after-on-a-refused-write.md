# 0031: A refused single-document write carries a Retry-After read from the write limiter

**Status:** Accepted after independent review of the change, 2026-11-24

## Context

The write limiter (spec `write-backpressure`) refuses a write while the object store is slow and computes a retry hint, but a single-document 429 carried it only in its message text. A client or proxy that backs off on `Retry-After` had nothing to read.

## Decision

- `WriteLimiter::retry_after(now)` returns the hint a refusal would give, from the same function the refusal uses, without admitting or refusing. The single-document routes add it as `Retry-After` to a 429.
- The hint is read after the refusal, not carried from it, so it may differ by the time between the two. Carrying it through every outcome type would touch each place a refusal is built, for a value that is only a hint.
- No switch: it is a header on an answer that is already a refusal, and sends no request.

## Consequences

- The limiter's decisions, settings and S3 request pattern are unchanged.
- A bulk still answers 200 with per-item 429s and carries no header.
- Rules out changing the refusal's decision to fit the header.
