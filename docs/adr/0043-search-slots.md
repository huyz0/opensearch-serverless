# 0043: Searches are admitted in order by a slot gate that shapes who waits, behind a setting

**Status:** Proposed

## Context

[ADR 0041](0041-tenant-cost-levels.md) names a level-2 lever that is not built: a tenant over budget searches in a low-priority lane. Today the only bound on searches is the REST server's fixed pool of blocking workers (`oss_rest` server, `workers` threads), which is also the hard bound after this change. A tenant with many concurrent searches can hold every worker while a quiet tenant's search queues in the accept queue. The shell's `SearchAdmission` answers this with a gate, a fair share and a short wait that never refuses.

This is not a memory bound: a search that runs still reserves from the query-execution pool and gets its 429 when that reservation fails ([ADR 0004](0004-bounded-memory-and-streaming.md)). The gate only decides who goes first under pressure.

## Decision

- `SearchSlots` lives in `oss-node` (`crates/node/src/search_slots.rs`) and is installed on `SearchService`; `SearchService::handle` takes a permit first and a guard gives it back when the response is sent, on every path including errors.
- Settings (node scope, `OSS_*` environment, read by `NodeConfig` like the tenant settings; not cluster settings): `OSS_SEARCH_SLOTS` (default 0: no gate, no code on the path), `OSS_SEARCH_FAIR_SHARE` (default 0.25), `OSS_SEARCH_WAIT_MS` (default 1,000), `OSS_SEARCH_MAX_WAITERS_PCT` (default 50, of the workers).
- State is one mutex over a small struct: `in_use`, `waiters`, and a map tenant to permits held, an entry removed at zero, so the map is bounded by the permits in use, which are bounded by the workers. The mutex is held only to read or change these counters, never across a wait body or a search. A fast path is not separate: the check and the add are in the same short critical section.
- Admission when `in_use < slots`: take a permit at once. Otherwise the gate is full, and the search is admitted without waiting unless it is of a tenant that is over its share, in which case it waits.
- Over its share: the tenant holds more than `max(1, floor(slots * fair_share))` permits and at least one other tenant is holding a permit or waiting; a tenant alone on the node never waits on itself. A tenant at level 2 or 3 in enforce mode is over its share when `in_use >= ceil(slots / 2)`, whatever it holds. Level 3 differs from 2 only in the write pacing of ADR 0041; the search lane is the same. In observe mode only the fair share applies.
- A waiter parks on a condvar with an absolute deadline (`OSS_SEARCH_WAIT_MS` from arrival) and re-checks its condition in a loop, so a spurious wake is harmless. A release calls `notify_all`; waiters re-check and the first to find the gate not full and itself eligible takes the permit. No order among waiters is promised.
- A waiter that reaches its deadline runs anyway, as an overflow runner: it holds no permit, the guard releases nothing for it, and it is counted. A release never wakes a waiter that is not owed one, and `in_use` is never decremented past zero (saturating). So permits held never exceed `slots`, and at most `workers - slots` searches run beyond them, all of them counted.
- Waiters cannot take the whole pool: when `waiters` reaches `OSS_SEARCH_MAX_WAITERS_PCT` of the workers, a further over-share search does not wait and runs at once as an overflow runner, so the rest of the workers stay free for tenants within their share.
- Cost to the noisy tenant: up to `OSS_SEARCH_WAIT_MS` added to each of its searches when the gate is full. Tenants within their share never wait.
- It is in memory, per node, with no store request, no timer and no cross-node state.
- The usage route reports `search_slots` with fixed-size counters only: slots, in use, waiters, searches that waited, searches that ran past the wait, searches that skipped waiting for the waiter cap, and the longest wait seen. Nothing is kept per tenant in the report.

## Consequences

- With `OSS_SEARCH_SLOTS` at 0 nothing changes; the existing search tests pin that, and new tests run with the gate on, so both modes run in CI.
- The gate orders and delays; it never refuses, so a search is never failed by a lane it did not choose, and the hard bounds stay the worker pool and the memory budget.
- Acceptance criteria go in `docs/specs/search-slots/SPEC.md`: a search takes and returns a permit on every path; a tenant over its share waits and one within its share does not; the wait ends at the deadline; the waiter cap stops waits piling up; a level-2 tenant in enforce mode waits at half the slots and in observe mode does not; the counters in the usage route; none of it with the setting at 0.
- It stays off by default until measured: search p99 with the gate on and off below the slots within 5%, a noisy tenant against a quiet one at saturation, no extra store requests (none are added), and a soak with searches failing mid-wait showing every permit returned.
