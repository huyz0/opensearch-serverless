# 0043: Searches take a slot of a bounded gate, and a tenant over its share waits briefly for one, behind a setting

**Status:** Proposed

## Context

[ADR 0041](0041-tenant-cost-levels.md) names a level-2 lever that is not built: a tenant over budget searches in a low-priority lane. Nothing bounds how many searches a node runs at once except the REST server's worker pool, so a tenant with many concurrent searches can hold every worker while a quiet tenant's search queues behind them. The shell's `SearchAdmission` answers both: a counting gate of slots, a fair share of them for one tenant while the gate is full, and a short wait, never a refusal.

## Decision

- A `SearchSlots` gate in the node holds `slots` permits (setting `OSS_SEARCH_SLOTS`, default 0 meaning off, no gate at all). A search takes one at the start of `SearchService::handle` and gives it back when the response is sent, including on error; the permit is a guard that drops.
- While fewer than `slots` are in use, every search takes a permit at once and the gate costs one atomic add.
- When the gate is full, a search of a tenant that already holds more than a quarter of the slots (`OSS_SEARCH_FAIR_SHARE`, default 0.25), or whose tenant is at level 2 or above in enforce mode, waits for a permit; any other search takes the next free one in arrival order.
- A level-2 tenant counts as full at half the slots, so the other half is left to tenants within budget.
- A wait is at most `OSS_SEARCH_WAIT_MS` (default 1,000). When it ends the search runs anyway: the gate orders searches under pressure and never refuses one, so a search is never failed by a lane it did not choose. The worker pool stays the hard bound.
- The gate is in memory and per node: no store request, no cross-node state, no lock held across a search. Waiting uses a condvar on a counter; a waiter is a worker already in the pool, so waiting adds no thread.
- Waits and the permits held are reported in the usage route next to the tenant levels (`search_slots`: slots, in use, waited, waited past the cap), so the effect is seen before it is relied on.

## Consequences

- With the setting off nothing changes. With it on, a search that waits is delayed by at most the cap, and a tenant within its share never waits while another is over its own.
- A search past the cap still runs, so under sustained overload the node can run more than `slots` searches (up to the workers); the gate shapes who goes first, not how many.
- Measured before any default changes: search p99 with the gate on and off at a load below the slots (must be within 5%) and a noisy tenant against a quiet one at saturation. Until then it stays off.
- Level 2 acts only in enforce mode, as the other levels do.
