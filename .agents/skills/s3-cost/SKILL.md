---
name: s3-cost
description: "WHAT: The S3 request-cost rules and how to prove a change keeps them. USE WHEN: touching crates/blob, adding or changing any object-store read, write or list, adding a timer or background job, or designing a write or read path."
---

# S3 cost

The bill is S3 requests. Rationale and numbers: [docs/principles.md](../../../docs/principles.md#cost-s3-requests-are-the-bill).

## Rules

- **Count requests per operation, retries included,** and state them in the PR. Requests scale with nodes and bytes, never documents, indices or elapsed time per index.
- **Cheapest first:** a key derived from data already held (free), then a GET or ranged GET, then a LIST (priced like a PUT, ≤1,000 keys). Every LIST carries `// LIST:` with the reason nothing cheaper works.
- **No extra PUTs.** Carry metadata in an upload that already happens. Never trade PUTs for an equal or smaller number of LISTs.
- **Metered:** every attempt calls `Meter::record` in `oss-blob`.
- **Idle is free:** nothing per index on a timer. A background job's cost scales with work done, not with indices held.
- **Depth:** no data-dependent chain of fetches on a user path; fan out within a round.

## Proving it

Write a test against the in-process emulator that asserts exact counts per request kind (`meter.count(S3Op::Put) == 1`, `meter.count(S3Op::List) == 0`). A request-count assertion is never loosened to make a change pass.

## Enforced by

- `scripts/check-s3-access.py` (client crates only in `oss-blob`; `// LIST:` on every LIST call).
- Count assertions in tests.
- Not mechanical: whether a request scales with the wrong thing. Review (`/quality-review`).
