# Asynchronous search: submit, get, delete, stats, as a native plugin

## Serves

The owner's request of 2026-11-28: port the stock asynchronous search plugin to Rust as a plugin on [ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md), results as node-local memory per [ADR 0084](../../adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md) (after [ADR 0072](../../adr/0072-tasks-are-node-local-memory-with-no-blob-requests.md)). No Java in the runtime path: Java is the oracle, through recorded stock answers, and a perf and memory comparison.

## Change

Five slices, one PR each, behind `serverless.async_search.enabled` (`OSS_ASYNC_SEARCH`), **default on** (the owner's rule of 2026-11-28: a plugin needs no enable flag, or the flag is on by default); `false` turns it off.

1. **Job capability** (prerequisite: plugin-system slices 1 and 2 merged, and the route mounting of #963). `oss-ext`: the `job` capability, its manifest key, policy grant and ceilings, the strict-parser change and the ABI minor bump; `oss-node`: the job registry (ADR 0072's mechanism in its own namespace), the bounds of ADR 0084, the relay, the search's cancel flag (added to the reader's search entry if it lacks one), and a test plugin that uses it. No route yet.
2. **Plugin core** (`plugins/async-search`): parse the submit's parameters (`wait_for_completion_timeout`, `keep_on_completion`, `keep_alive`, the search's own) and body, the state machine, and the response shape; `POST [/{index}]/_plugins/_asynchronous_search` and `GET|DELETE .../{id}`, with the legacy prefix and the index-leading form. Until the plugin-system `search` capability lands, the job runs through the node's in-process search entry behind the same trait (AC-16).
3. **Wait, keep and expiry:** `wait_for_completion_timeout` (default 1 s, at most 10 s; on submit and on `GET`), `keep_on_completion`, `keep_alive` on submit and on `GET`, `max_running_time`, expiry on access, `CLOSED`.
4. **Stats:** `GET /_plugins/_asynchronous_search/stats` for this node.
5. **Recording, conformance, measurement, docs, the flip decision.**

Not here: a system index and anything written to the store, cluster-wide stats, `_cat`, the stock plugin's cluster settings other than those in the Switch section, Java-shell interop. Refusals found before the job starts (parameters, body parse, index resolution) are synchronous with their usual status; an error while the search runs is answered as the recording shows (AC-3).

Stock shapes are the recording of stock OpenSearch 3.10.0 in [`fixtures/async-search/async-search.json`](../../../fixtures/async-search/async-search.json) (`record.py`, ids and times as placeholders). A rule marked *provisional* is one the recording does not cover.

**States as the recording shows them.** `RUNNING`: the search has not finished. `CLOSED`: it finished inside the submit's wait and was not kept; the submit answers it with an `id`, and a later read or delete of that id is 404. `PERSISTING`: it finished inside the wait and `keep_on_completion` is true. `STORE_RESIDENT`: a kept result read later. There is no `SUCCEEDED` or `FAILED`. Results are held in node memory (ADR 0084); the persisted names are reported for them because clients read those names, and `compatibility.md` says so.

Settings (registered per [add-a-setting](../../how-to/add-a-setting.md)): `serverless.async_search.enabled`, `max_running` (8), `max_running_per_tenant` (2), `max_kept` (64), `max_kept_bytes` (32 MiB), `max_response_bytes` (1 MiB), `max_keep_alive` (1 h), `max_running_time` (10 min). Differences from stock (8 running against stock's 20, 1 MiB, 1 h against 5 days, no persisted states, this node's stats, memory-only results) go to `docs/compatibility.md` in slice 5.

File ownership: new `docs/specs/async-search/`, `plugins/async-search/`, the `job` types in `crates/ext`, `crates/node/src/jobs/`, an assembly hook. Nothing in the files of the other plugin ports.

## Acceptance criteria

- AC-1: a submit whose search finishes inside the wait answers 200 with `id`, `state: CLOSED`, `start_time_in_millis`, `expiration_time_in_millis` and the search's response under `response`, the same hits and aggregations as a synchronous `_search` of the same body (replayed against the recording); with `keep_on_completion=true` the state is `PERSISTING`; a malformed or absent body is a match-all search.
- AC-2: a search held on a gate past the wait answers 200 `state: RUNNING` with an `id` and a partial `response` (`took`, `timed_out`, `num_reduce_phases` 0, `_shards` with the total and `successful` 0, empty `hits`); `GET` answers `RUNNING` until the gate opens, then `STORE_RESIDENT` with the response when the job is kept, and 404 when it is not; a read never restarts the work; the wait is a condvar wait (a mutation that polls fails the wakeup count).
- AC-3: a query the reader refuses at parse (`parsing_exception`) and a bad parameter (`keep_alive`, `wait_for_completion_timeout`, `keep_on_completion`, an unknown name: 400 `illegal_argument_exception` with the recording's messages) are synchronous 400s and create no entry; an error while the search runs (a missing index, a shard failure) is 200 with `state: CLOSED` and `error` in place of `response` (*provisional* for the kept variant); a `GET` with no id is 405; `{node}/stats` is 400 no handler.
- AC-4: `keep_on_completion=false` (default): a job finished inside the wait leaves no entry (`GET` and `DELETE` are 404 `resource_not_found_exception`, "Either the resource [id] does not exist or you do not have access"); a job still running when submit returned stays readable as `RUNNING` and, once finished, is dropped (404). `keep_on_completion=true` keeps the finished result as `STORE_RESIDENT` until `keep_alive` ends. The registry gains the `keep` flag in slice 3.
- AC-5: `keep_alive` on submit and on `GET` sets the expiry; below 1 minute is 400 `action_request_validation_exception` ("[keep_alive] must be greater than 1 minute"); over `max_keep_alive` is 400 `illegal_argument_exception` ("Keep alive for asynchronous search (<ms>) is too large. It must be less than (<max>)", naming this node's setting); the default is the smaller of 12 hours and the maximum; an entry past expiry is 404 on access; expiry runs on access only and the test counts that no thread or timer exists.
- AC-6: `DELETE` of a running job cancels it and answers 200 `{"acknowledged":true}`, and the id is then 404; the slot is freed when the search's thread exits, not before (a test holds the thread and sees the slot still counted); `DELETE` of a kept job drops it the same way; of an unknown, malformed or expired id, 404 with the same message.
- AC-7: a 9th running job, or a 3rd for one tenant, or a submit by a tenant at its kept cap with all entries running, is 429 `es_rejected_execution_exception` with no entry; more than 64 kept, 32 MiB, or half of either for one tenant evicts that tenant's oldest finished first, never a running one; a response over `max_response_bytes` is cut off by the counting writer and the job ends with the error `response_too_large` (the answer carries `error` in place of `response`) (on a waiting submit as on a read), never held whole; a refused pool reservation is 429.
- AC-8: memory: a running job holds `max_response_bytes` and a kept one its size plus 1 KiB from the `Extensions` pool, released when the job's thread exits (finish, panic, or after a delete or `max_running_time` cancel; evict and expiry apply to finished entries only), the pool reads 0 held when the entries are gone (every add or read sweeps the expired), and the peak with every limit reached, including the calls' own reservations (16 x (1 MiB body + 4 MiB response ceiling)), equals the sum stated in ADR 0084 (80 + 8 + 32 MiB + 72 KiB) and is held to it by the pool's accounting.
- AC-9: a job whose thread panics frees its slot and reservation and answers with an `error` in place of `response`; ids start from the process start time and never repeat after a restart (the old id is 404).
- AC-10: an id naming another node is answered by that node over the transport (read, extend, delete), relayed with its status and body and with the verified tenant and user, a hop the target never repeats; no live lease for the node is 404 naming it; a malformed id is 400.
- AC-11: stats answer from memory in the recording's shape: `_nodes` (`total` 1, `successful` 1, `failed` 0), `cluster_name`, and `nodes.<id>.asynchronous_search_stats` with `submitted`, `initialized`, `running_current`, `persisted`, `search_failed`, `search_completed`, `rejected`, `persist_failed`, `cancelled`; `expired`, `evicted` and `held_bytes` go under a separate `serverless` key; no store request.
- AC-12: isolation: another tenant, or another user of the same tenant, reading, extending or deleting a job gets 404 (existence is not leaked); a job's search runs with the submitter's identity and index scope.
- AC-13: **cost per operation, by the request meter on the emulator:** submit adds 0 requests beyond its search's (a single-index cold search 3 GET at depth 3, warm 0, read from [request-budgets](../../request-budgets.md); a wildcard or no index costs what that search costs); get, extend, delete, stats and a relayed read 0 PUT, 0 GET, 0 LIST; an idle node with entries held makes 0 requests and, over an hour of virtual-clock wakeups as `virtual-time-soak` does, starts no thread or timer (a mutation adding a sweeper thread fails it).
- AC-14: routes and policy: the plugin claims only `/_plugins/_asynchronous_search`, `/_opendistro/_asynchronous_search` and `/{index}/_plugins/_asynchronous_search` (and `/{index}/_opendistro/...`), a planted built-in `/{index}/...` route does not collide and is not shadowed, every route is in the action table (`all_access` unless mapped), and with `serverless.async_search.enabled` set to false, or `serverless.plugins.enabled` off, the prefixes go to a configured plugin upstream and no `job` is granted.
- AC-15: **against Java (provisional, a stock node as recorded in fixtures/async-search, same corpus, 200 repetitions):** the median latency of a trivial submit that finishes inside the wait is not more than 10% worse, and resident memory with 64 kept 1 MiB results is not above the Java node's; recorded in VERIFIED.md, `NOT-RUN` with the reason if no stock node can be built, and the default flip stays blocked while it is `NOT-RUN`.
- AC-16: the capability: a plugin without the `job` grant has no job API and one without `identity` cannot record a submitter (both grants named in the policy); `job` carries the manifest key, the policy grant and ceilings, the ABI minor bump, a golden layout test of its types and the version rule both ways (plugin-system AC-26); a submit is charged one `searches` unit of its call's budget and a second past the budget is 429 `plugin_budget_exceeded`; the in-process search entry and the `search` capability give the same answers for the same request.
- AC-17: waits and bulkhead (16 per plugin, 6 per tenant): two waiting submits from one tenant leave room for a `GET`, a `DELETE` and a stats call from that tenant, each answered within its deadline, and the same with a relayed waiting read over the in-process transport; a wait over 10 s is 400 (stock's limit is not in the recording); a job past `max_running_time` is cancelled by the search's own deadline check and answers with the error `search_timeout`, with nobody reading it in between, and the search's cancel flag is read by the reader between its stages (cited in the slice 1 PR).
- AC-18: the by-query tasks and `GET /_tasks` tests pass unchanged, a job id is not answered by `GET /_tasks/{id}` and a task id is not answered by the async routes.

## Test plan

- AC-1, 3: first against `_search` in the rig, then recorded stock answers under `fixtures/async-search/` replayed through `crates/compat` when the recording exists; each seen red against a plugin that returns the search body unwrapped.
- AC-11, 14, 16, 17, 18: table-driven policy and routes, the golden layout test, a gate-held search with two waiting submits and a delete, and the existing task suites.
- AC-2, 4, 5, 6: `crates/node/tests/server/async_search.rs` over the by-query rig with the search held on a gate the test opens, and an injected clock.
- AC-7, 8: the pool's accounting and the registry's counters at every limit; a mutation that forgets a release on one exit fails the held-bytes assertion.
- AC-9: a search stub that panics.
- AC-10: two nodes on the in-process transport, as `tasks_route.rs` is.
- AC-13: the request meter, with its rows added to the budget table and read from it, not repeated; a virtual clock over an hour with `thread` count before and after.
- AC-14: the existing completeness test and a table-driven policy.
- AC-15: a measurement run, `NOT-RUN` in VERIFIED.md until a stock node is available.

## S3 request budget

Per operation, worst case, each a row in [request-budgets](../../request-budgets.md) with its test when the slice lands.

| Operation | PUT | GET | LIST | Depth |
|---|---|---|---|---|
| Submit (the search's own cost) | 0 | cold index 3, warm 0 | 0 | cold 3, warm 0 |
| Get, extend | 0 | 0 | 0 | 0 |
| Delete, cancel | 0 | 0 | 0 | 0 |
| Stats | 0 | 0 | 0 | 0 |
| A relayed read or delete | 0 | 0 (a lease already held) | 0 | 0 |
| Idle with entries held | 0 | 0 | 0 | 0 |

No request scales with documents, indexes, entries or elapsed time.

## Switch

`serverless.async_search.enabled`, node scope, **default on** (owner's rule, 2026-11-28); `OSS_ASYNC_SEARCH=false` turns it off and the prefixes go to a plugin upstream when one is configured. On by default has a bar that is measured, not assumed: VERIFIED.md records AC-13's counts, AC-8's peak and AC-15's comparison, and the principles' bar holds: no extra requests and no search regression with the plugin system on and the registry empty. A plugin reaches a node only when the plugin system's own policy names it. The compatibility differences are listed as differences. The limits are settings, not switches. The `job` capability has no switch of its own: nothing grants it to a plugin whose policy does not ask.

## Risks

- A result is lost with its node and read through it; clients that expect stock's persistence (a poll after a restart, a long `keep_alive`) get 404. Documented in `compatibility.md`.
- The `job` capability touches the plugin contract and the ADR 0072 registry, both shared with other threads; slice 1 is kept to new files and a registry extension that leaves by-query tasks' tests unchanged.
- The `search` capability (plugin-system slice 4) may not exist when slice 2 starts; AC-16 holds the fallback and the capability to the same answers.
- The `Extensions` pool is 0% by default: until it is sized, every call is refused 429, which is why the plugin needs a sized pool; the default-on flag does not change that.
- The recording does not cover a kept search that fails while running, the largest `wait_for_completion_timeout` stock accepts, or a second node; those rules are *provisional*.
