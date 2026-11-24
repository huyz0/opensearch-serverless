# 0046: Searches are admitted in order by a slot gate that shapes who waits, behind a setting

**Status:** Proposed

## Context

[ADR 0041](0041-tenant-cost-levels.md) names a level-2 lever that is not built: a tenant over budget searches in a low-priority lane. Today the only bound on searches is the REST server's fixed pool of blocking workers (`oss_rest` server, `workers` threads), which is also the hard bound after this change. A tenant with many concurrent searches can hold every worker while a quiet tenant's search queues in the accept queue. The shell's `SearchAdmission` answers this with a gate, a fair share and a short wait that never refuses.

This is not a memory bound: a search that runs still reserves from the query-execution pool and gets its 429 when that reservation fails ([ADR 0004](0004-bounded-memory-and-streaming.md)). The gate only decides who goes first under pressure.

## Decision

- `SearchSlots` lives in `oss-node` (`crates/node/src/search_slots.rs`) and is installed on `SearchService`; `SearchService::handle` takes a permit first and a guard gives it back when the response is sent, on every path including errors. It is built with the REST `workers` count from the node config, and with a closure `tenant level for an index` over the tenant levels of ADR 0041 (a tenant is the index's bucket group, else the index). Level 4 is read as level 3. The level is read once on arrival, before the gate's mutex is taken, and is fixed for that search's wait; the closure takes no lock the gate holds and never calls the gate. A search names one tenant: the one of its first resolved index, and a search that resolves to no index (or whose tenant is unknown) is not gated.
- Settings (node scope, `OSS_*` environment, read by `NodeConfig` like the tenant settings; not cluster settings): `OSS_SEARCH_SLOTS` (default 0: no gate, no code on the path), `OSS_SEARCH_FAIR_SHARE` (default 0.25), `OSS_SEARCH_WAIT_MS` (default 1,000), `OSS_SEARCH_MAX_WAITERS_PCT` (default 50). With `OSS_SEARCH_SLOTS` at 0 none of these are checked. Otherwise a start refuses `slots + waiter cap` that is not below `workers` (so permit holders and waiters never fill the pool, and the workers left are the ones a within-share search and overflow runners use; the gate helps only where that headroom exists), a fair share outside (0, 1], and a percentage outside 0 to 100. The waiter cap is `ceil(workers * pct / 100)`, at least 1 when the percentage is above 0.
- State is one mutex over: `in_use`, `waiting`, and a map tenant to (permits held, waiting). An entry is removed when both are zero, so the map is bounded by the permits plus the waiters, which are bounded by the workers. The mutex is held only to read or change these counters, never across a search; the condvar wait releases it while parked.
- Admission, in this order, one critical section:
  1. If `in_use < slots` and the level rule does not hold for this tenant, take a permit.
  2. Otherwise, if the tenant is not over its share, run as an overflow runner.
  3. Otherwise, if `waiting` is at the waiter cap, run as an overflow runner; else wait, then take a permit when eligible, or run as an overflow runner at the deadline.
  So the fair-share rule changes behaviour only when the gate is full.   The level rule holds when the tenant is at level 2 or 3 in enforce mode and `in_use >= ceil(slots / 2)`; it applies below full, so the other half is left to tenants within budget, and is never evaluated in observe mode. A tenant is over its share when it holds, counting the arriving search, more than `max(1, floor(slots * fair_share))` permits and another tenant holds a permit or waits (`in_use > own_held || waiting > own_waiting`), so a tenant alone never waits on itself, or when the level rule holds. Level 3 differs from 2 only in the write pacing of ADR 0041.
- A waiter parks on a condvar with an absolute deadline (`OSS_SEARCH_WAIT_MS` from arrival) and re-checks in a loop, so a spurious wake is harmless. Eligible means: `in_use < slots` and the level rule does not hold. Releasing a permit decrements `in_use` and the tenant's count and calls `notify_all`; the first waiter to find itself eligible takes the permit. No order among waiters is promised.
- An overflow runner holds no permit, has no map entry and does not count toward its tenant's share: the share rule sees permits only, an accepted limit, because the runners are bounded by the workers and counted. Its guard releases nothing and notifies nothing. So permits held never exceed `slots`, and `in_use` is never decremented below zero.
- Cost to the noisy tenant: up to `OSS_SEARCH_WAIT_MS` added to each of its searches when it is over its share. A tenant within its share never waits.
- It is in memory, per node, with no store request, no timer and no cross-node state.
- The usage route reports `search_slots` with fixed-size counters only: slots, in use, waiting, searches that waited, searches that ran past the wait, searches that ran at once as overflow because the gate was full or the waiter cap was reached (one counter each), and the longest wait seen. Nothing is kept per tenant in the report.

## Consequences

- With `OSS_SEARCH_SLOTS` at 0 nothing changes; the existing search tests pin that, and new tests run with the gate on, so both modes run in CI.
- The gate orders and delays; it never refuses, so a search is never failed by a lane it did not choose, and the hard bounds stay the worker pool and the memory budget.
- Acceptance criteria go in `docs/specs/search-slots/SPEC.md`: a search takes and returns a permit on every path; a tenant over its share waits and one within its share does not; the wait ends at the deadline; the waiter cap stops waits piling up; a level-2 tenant in enforce mode waits at half the slots and in observe mode does not; the counters in the usage route; a limit test with the map at `slots` entries and the waiters at the cap; none of it with the setting at 0.
- It stays off by default until measured: search p99 with the gate on and off below the slots within 5%, a noisy tenant against a quiet one at saturation, no extra store requests (none are added), and a soak with searches failing mid-wait showing every permit returned.
