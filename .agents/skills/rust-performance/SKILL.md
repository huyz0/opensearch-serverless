---
name: rust-performance
description: "WHAT: How to optimise: measure first, then the Rust-first design rules. USE WHEN: optimising a hot path (per document, per query, per request, per byte), changing an in-memory representation, or reviewing an optimisation."
---

# Performance

## Rules

- **Measure, change one thing, measure again, keep the numbers** in the PR (hardware, workload, before and after, provisional when on a shared host or emulator).
- **Where a number can be asserted** (requests per call, bytes, an allocation bound), assert it in a test.
- **Allocation is budgeted:** pooled fixed-size buffers and arenas on request paths; reserve from the node memory budget first (ADR 0004).
- **Stream, do not materialise:** bounded chunks, never a whole bulk, segment or result set in memory when a stream will do.
- **Fewer copies:** borrow, parse in place, reuse buffers, size collections up front. A copy on a hot path needs a reason.
- **Monomorphise per-document loops;** `dyn` only at coarse boundaries.
- **Ownership over locks:** immutable shared readers (`Arc`), one owner for mutable state, `Mutex` only on control-plane state.
- **SIMD only where a profile shows it pays,** with a scalar version tested against it on x86-64 and ARM64.
- **Keep the simple version** unless the measured gain justifies the complexity.

## Enforced by

- Nothing mechanical: performance regressions do not fail tests. Review rejects an unmeasured performance claim.
