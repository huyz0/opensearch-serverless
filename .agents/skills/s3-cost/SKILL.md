---
name: s3-cost
description: "WHAT: How to prove a change keeps the S3 request-cost rules. USE WHEN: touching crates/blob, adding or changing any object-store read, write or list, adding a timer or background job, or designing a write or read path."
---

# S3 cost

The bill is S3 requests. The rules, once: [principles.md, Cost](../../../docs/principles.md#cost-s3-requests-are-the-bill). The steps for a new call: [how-to/add-an-s3-call.md](../../../docs/how-to/add-an-s3-call.md).

## Procedure

1. Before writing the call, answer in order: can the key be derived from data already held? Can the information ride in an upload that already happens? Only then a GET, and only then a LIST with a `// LIST:` reason.
2. Count the operation's requests per kind, retries included, and say what they scale with. Nodes and bytes pass; documents, indices or time per index fail.
3. Write a test against the in-process emulator asserting the exact counts (`meter.count(S3Op::Put) == 1`, `meter.count(S3Op::List) == 0`). A count assertion is never loosened to make a change pass.
4. State the counts per operation in the PR's Evidence.

## Enforced by

- `scripts/check-s3-access.py` (client crates only in `oss-blob`; `// LIST:` on every LIST call).
- The count assertions in tests.
- Not mechanical: whether a request scales with the wrong thing. Review (`/quality-review`).
