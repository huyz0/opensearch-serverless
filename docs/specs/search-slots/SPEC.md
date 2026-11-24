# Search slots

## Serves

[ADR 0047](../../adr/0047-search-slots.md), the level-2 lane of [ADR 0041](../../adr/0041-tenant-cost-levels.md): a per-node gate that orders searches under pressure and never refuses one. Roadmap R13.4.

## Change

- `oss_node::search_slots::SearchSlots` (`crates/node/src/search_slots.rs`): the gate of the ADR. `admit(tenant, level)` returns a guard; the guard gives a permit back on drop. Rules, state, counters and the overflow runner are the ADR's.
- `SearchService::set_search_slots` installs it; `handle` admits a search by the tenant and level of the first plain index name in its path, before the search runs (a search with no plain index name, a wildcard, or a point in time is not gated).
- Settings (node scope, read at boot; `OSS_SEARCH_SLOTS` 0 is off and nothing is installed): `OSS_SEARCH_SLOTS` (0, up to 10,000), `OSS_SEARCH_FAIR_SHARE_PCT` (25, 1 to 100: the ADR's fair share as a percentage), `OSS_SEARCH_WAIT_MS` (1,000, up to 60,000), `OSS_SEARCH_MAX_WAITERS_PCT` (50, 0 to 100). A start is refused when `slots + waiter cap` is not below the REST server's workers (128 by default).
- The usage route reports `search_slots` (slots, in use, waiting, admitted, waited, past the wait, overflow because full, overflow at the waiter cap, longest wait in ms) when the gate is on.
- The level of a tenant comes from the tenant levels of ADR 0041 (0 in observe mode, and 0 when levels are off); the tenant is the index's group or the index.

## Acceptance criteria

- AC-1: a search takes a permit and gives it back when its guard drops, on every path; `in_use` returns to 0 and the tenant entry is removed.
- AC-2: with the gate not full and no level rule, every search takes a permit at once; with the gate full, a tenant within its share runs at once as an overflow runner (no permit, counted `overflow_full`).
- AC-3: with the gate full, a tenant over its share while another tenant is active waits, and a quiet tenant arriving meanwhile does not.
- AC-4: a wait ends at `OSS_SEARCH_WAIT_MS`: the search runs as an overflow runner and `past_wait` counts it.
- AC-5: with the waiter cap reached, a further over-share search does not wait and runs as an overflow runner (`overflow_cap`).
- AC-6: a tenant at level 2 or 3 waits once `in_use >= ceil(slots / 2)` while another tenant is active, below the full gate; at level 0 (observe mode) it does not; alone, at any level, it never waits.
- AC-7: a released permit wakes a waiter that is then eligible, before its deadline.
- AC-8: the report counts waits, past-wait runs, overflows and the longest wait.
- AC-9: with `slots` tenants holding permits and the waiters at the cap, the state holds one entry per tenant and no more.
- AC-10: the settings default to off, 25, 1,000 and 50, refuse text and out-of-range values and a start with `slots + waiter cap` not below the workers.
- AC-11: a booted node with `OSS_SEARCH_SLOTS` set serves searches as before and reports `search_slots` on the usage route; with it unset the report has none.

## Test plan

`crates/node/tests/search_slots.rs` (AC-1 to AC-9, the gate alone with threads), `crates/node/tests/server/boot_search_slots.rs` (AC-10, AC-11); each seen failing with the behaviour removed.

## S3 request budget

Unchanged: the gate is in memory.

## Switch

`OSS_SEARCH_SLOTS`, default 0 (off). The default stays off until the measurements in the ADR are made.
