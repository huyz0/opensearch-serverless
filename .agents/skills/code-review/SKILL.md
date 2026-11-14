---
name: code-review
description: "WHAT: The self-review checklist run before declaring a change done. USE WHEN: finishing a unit of work, before committing or opening a PR, or reviewing a diff."
---

# Code review / self-review

The implementing agent self-reviews, then runs `/quality-review` for an independent read. A change that cannot truthfully tick these is not ready.

## Checklist

- **Gate green:** `scripts/gate.sh` ran and passed, and you saw it pass.
- **S3 cost:** request counts per operation stated and asserted in a test; no new LIST without `// LIST:`; no new PUT that an existing upload could carry; nothing per index on a timer (`s3-cost` skill).
- **Switches:** new behaviour sits behind a setting or feature unless measured with no downside; tests cover both values; a proven switch is removed.
- **Memory:** request paths stream in bounded chunks from pooled buffers, reserve from the node budget, use bounded channels, and mark each remaining allocation `// ALLOC: <bound>` (ADR 0004). A peak-memory assertion covers new load paths.
- **Bounds:** every buffer, queue, cache, batch, fan-out and retry loop has a bound checked before allocation, a stated failure mode (refuse, spill, back-pressure), and a test at the limit and one past it.
- **Structure:** layering intact, one concern per module, nothing public that need not be (`architecture` skill).
- **Arithmetic** on lengths, counts or offsets from storage is checked; an allow carries an `// ARITH:` proof.
- **Tests** assert properties, include a negative case, and would fail if the code were wrong (`test-coverage` skill).
- **Docs** in the same change: crate README, `docs/architecture.md`, a how-to or an ADR when the change alters what they say.
- **Comments** describe what the code does; no task or milestone ids.
- **No scratch files** left behind.

## Review budget

Two review rounds nominal, three at most. A blocking finding in round two means split the change, not a fourth round. Minor findings do not block; note them in the PR.

## Enforced by

- `scripts/gate.sh` for everything mechanical ([docs/mechanical-gates.md](../../../docs/mechanical-gates.md)).
- `/quality-review` (the `quality-reviewer` subagent) for cost scaling, switch discipline, bounds and test strength, which no script checks.
