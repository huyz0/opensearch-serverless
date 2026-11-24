# Tenant levels

## Serves

Roadmap R13.4, the first piece: the policy of the shell's `Guardrails` (levels from cost against a budget, and write pacing at level three), over the usage ledger of [store-usage](../store-usage/SPEC.md). [ADR 0038](../../adr/0038-tenant-cost-levels.md).

## Change

- `oss_blob::guard::Guardrails`, sans-IO: the caller names the time, the budgets and each tenant's cumulative cost and documents at every tick; the policy answers levels, pacing waits and reports. Nothing in it reads a clock, a setting or the store.
- Levels: over budget for a minute, level 1; two more minutes still over, each next level to 3; under budget for five minutes, one level off. A level set from outside (`tenant=level,...`) holds when higher; a refusal shows as level 4 for a minute.
- `pace_write`: for an enforced tenant at level 3 and over budget, a write is held until it fits the documents a second the budget allows (a burst of 4 s of it), or refused when the wait would pass the longest wait.
- A tenant a tick does not name is forgotten (memory is bounded by the ledger's lines).
- Not in this change: wiring into the node (tenant from the index's bucket group, budgets by tier, the timer, publish stretch, search admission, settings), the non-store counters, the collector. Nothing in the node calls it yet.

## Acceptance criteria

- AC-1: a tenant 20% over budget reaches level 1 after a minute, 2 after two minutes more, 3 after two more, and stays at 3.
- AC-2: a tenant under budget stays at 0; one that falls back under loses a level after five minutes.
- AC-3: a level set from outside holds when higher than the local one; a pair that does not parse, or names a level above 4, is left out.
- AC-4: a refusal shows as level 4 for a minute, and counts.
- AC-5: at level 3, enforced, a write within the burst waits 0 ms, a write past it waits what the bucket lacks, and one that would wait over the longest wait is refused.
- AC-6: observe and off modes, a write of no documents and a tenant not seen are never paced.
- AC-7: a tenant a tick does not name is forgotten; one set from outside keeps its level.

## Test plan

`crates/blob/tests/guard.rs`, seen failing with the first-level wait halved, the level-down wait cut to 100 s and the longest wait times ten.

## S3 request budget

Unchanged: no request, no store.

## Switch

Nothing calls it yet. The node's switch (mode off, observe or enforce; default observe, as the shell) comes with the wiring.

## Risks

- A tenant spread over several nodes shows only this node's share, so the collector's level matters; the policy takes it as set.
- Pacing holds a thread: the caller must sleep outside any lock.
