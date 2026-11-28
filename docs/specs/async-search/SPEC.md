# Asynchronous search: submit, get, delete, stats, as a native plugin

## Serves

The owner's request of 2026-11-28: port the stock asynchronous search plugin to Rust as a plugin on [ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md), results as node-local memory per [ADR 0084](../../adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md) (after [ADR 0072](../../adr/0072-tasks-are-node-local-memory-with-no-blob-requests.md)). No Java in the runtime path: Java is the oracle, through recorded stock answers, and a perf and memory comparison.

## Change

Five slices, one PR each, behind `serverless.async_search.enabled` (`OSS_ASYNC_SEARCH`), default off.

1. **Job capability** (prerequisite: plugin-system slices 1 and 2 merged, and the route mounting of #963). `oss-ext`: the `job` capability, its manifest key, policy grant and ceilings, the strict-parser change and the ABI minor bump; `oss-node`: the job registry (ADR 0072's mechanism in its own namespace), the bounds of ADR 0084, the relay, the search's cancel flag (added to the reader's search entry if it lacks one), and a test plugin that uses it. No route yet.
2. **Plugin core** (`plugins/async-search`): parse the submit's parameters (`wait_for_completion_timeout`, `keep_on_completion`, `keep_alive`, the search's own) and body, the state machine, and the response shape; `POST [/{index}]/_plugins/_asynchronous_search` and `GET|DELETE .../{id}`, with the legacy prefix and the index-leading form. Until the plugin-system `search` capability lands, the job runs through the node's in-process search entry behind the same trait (AC-16).
3. **Wait, keep and expiry:** `wait_for_completion_timeout` (default 1 s, at most 10 s; on submit and on `GET`), `keep_on_completion`, `keep_alive` on submit and on `GET`, `max_running_time`, expiry on access, `CLOSED`.
4. **Stats:** `GET /_plugins/_asynchronous_search/stats` for this node.
5. **Recording, conformance, measurement, docs, the flip decision.**

Not here: persisted states and a system index, cluster-wide stats, `_cat`, the stock plugin's cluster settings other than those in the Switch section, Java-shell interop. Refusals found before the job starts (parameters, body parse, index resolution) are synchronous with their usual status; an error while the search runs makes the job `FAILED`.

Stock shapes are from the plugin's published documentation, so provisional (each rule below marked *provisional* is the expected answer, which the recording confirms or flips) until slice 5 records a stock node (`fixtures/async-search/record.sh`, an OpenSearch with the plugin installed; if the plugin artifact cannot be fetched here, the recording step is `NOT-RUN` and says so).

Settings (registered per [add-a-setting](../../how-to/add-a-setting.md)): `serverless.async_search.enabled`, `max_running` (8), `max_running_per_tenant` (2), `max_kept` (64), `max_kept_bytes` (32 MiB), `max_response_bytes` (1 MiB), `max_keep_alive` (1 h), `max_running_time` (10 min). Differences from stock (8 running against stock's 20, 1 MiB, 1 h against 5 days, no persisted states, this node's stats, memory-only results) go to `docs/compatibility.md` in slice 5.

File ownership: new `docs/specs/async-search/`, `plugins/async-search/`, the `job` types in `crates/ext`, `crates/node/src/jobs/`, an assembly hook. Nothing in the files of the other plugin ports.

## Acceptance criteria

- AC-1: submit with a search that finishes inside the wait answers 200 with `state: SUCCEEDED`, the search's response under `response`, `start_time_in_millis`, `expiration_time_in_millis`, and an `id` when `keep_on_completion` is true, with the same hits and aggregations as a synchronous `_search` of the same body (compared against `_search` in the test, and on recorded fixtures when they exist).
- AC-2: a search held on a gate past the wait answers 200 `state: RUNNING` with an `id` and no `response`; `GET` answers `RUNNING` until the gate opens and then `SUCCEEDED` with the response; a read never restarts the work; the wait is a condvar wait (a mutation that polls fails the wakeup count).
- AC-3: an error while the search runs (a script error, a shard failure) gives `state: FAILED` with `error` in stock's shape; a refusal found before the job starts (parameter, body parse, missing index, switch off, over a limit) keeps its synchronous status and creates no entry.
- AC-4 (*provisional*): `keep_on_completion=false` (default) and the job finished inside the wait: no entry, `GET` is 404 `resource_not_found_exception`; a job still running when submit returned stays readable until it finishes, then for `keep_alive`; `keep_on_completion=true` keeps it for `keep_alive`.
- AC-5: `keep_alive` on submit and on `GET` sets the expiry; a value over `max_keep_alive` is 400; the default is the smaller of 12 hours and the maximum; an entry past expiry is 404 on access; expiry runs on access only and the test counts that no thread or timer exists.
- AC-6: `DELETE` of a running job cancels it and answers `{"acknowledged":true}`; the slot is freed when the search's thread exits, not before (a test holds the thread and sees the slot still counted), and a waiting caller of that job is answered `CLOSED` (*provisional*); `DELETE` of a finished job drops it; of an unknown or expired id, 404.
- AC-7: a 9th running job, or a 3rd for one tenant, or a submit by a tenant at its kept cap with all entries running, is 429 `es_rejected_execution_exception` with no entry; more than 64 kept, 32 MiB, or half of either for one tenant evicts that tenant's oldest finished first, never a running one; a response over `max_response_bytes` is cut off by the counting writer and the job is `FAILED` with `response_too_large` (on a waiting submit as on a read), never held whole; a refused pool reservation is 429.
- AC-8: memory: a running job holds `max_response_bytes` and a kept one its size plus 1 KiB from the `Extensions` pool, released when the job's thread exits (finish, panic, or after a delete or `max_running_time` cancel; evict and expiry apply to finished entries only), the pool reads 0 held when the entries are gone (every add or read sweeps the expired), and the peak with every limit reached, including the calls' own reservations (16 x (1 MiB body + 4 MiB response ceiling)), equals the sum stated in ADR 0084 (80 + 8 + 32 MiB + 72 KiB) and is held to it by the pool's accounting.
- AC-9: a job whose thread panics frees its slot and reservation and reads `FAILED`; ids start from the process start time and never repeat after a restart (the old id is 404).
- AC-10: an id naming another node is answered by that node over the transport (read, extend, delete), relayed with its status and body and with the verified tenant and user, a hop the target never repeats; no live lease for the node is 404 naming it; a malformed id is 400.
- AC-11 (*provisional*): stats answer from memory in stock's shape (`_nodes`, `cluster_name`, `nodes.<id>.asynchronous_search_stats` with stock's counter names `running_current`, `submitted`, `initialized`, `search_completed`, `search_failed`, `persisted`, `persist_failed`, `rejected`, `cancelled`), plus under a separate `serverless` key `expired`, `evicted`, `held_bytes`; `_nodes.total` is 1; no store request.
- AC-12: isolation: another tenant, or another user of the same tenant, reading, extending or deleting a job gets 404 (existence is not leaked); a job's search runs with the submitter's identity and index scope.
- AC-13: **cost per operation, by the request meter on the emulator:** submit adds 0 requests beyond its search's (a single-index cold search 3 GET at depth 3, warm 0, read from [request-budgets](../../request-budgets.md); a wildcard or no index costs what that search costs); get, extend, delete, stats and a relayed read 0 PUT, 0 GET, 0 LIST; an idle node with entries held makes 0 requests and, over an hour of virtual-clock wakeups as `virtual-time-soak` does, starts no thread or timer (a mutation adding a sweeper thread fails it).
- AC-14: routes and policy: the plugin claims only `/_plugins/_asynchronous_search`, `/_opendistro/_asynchronous_search` and `/{index}/_plugins/_asynchronous_search` (and `/{index}/_opendistro/...`), a planted built-in `/{index}/...` route does not collide and is not shadowed, every route is in the action table (`all_access` unless mapped), and with `serverless.async_search.enabled` off, or `serverless.plugins.enabled` off, the prefixes go to a configured plugin upstream and no `job` is granted.
- AC-15: **against Java (provisional, recorded stock node, same corpus, 200 repetitions):** the median latency of a trivial submit that finishes inside the wait is not more than 10% worse, and resident memory with 64 kept 1 MiB results is not above the Java node's; recorded in VERIFIED.md, `NOT-RUN` with the reason if no stock node can be built, and the default flip stays blocked while it is `NOT-RUN`.
- AC-16: the capability: a plugin without the `job` grant has no job API and one without `identity` cannot record a submitter (both grants named in the policy); `job` carries the manifest key, the policy grant and ceilings, the ABI minor bump, a golden layout test of its types and the version rule both ways (plugin-system AC-26); a submit is charged one `searches` unit of its call's budget and a second past the budget is 429 `plugin_budget_exceeded`; the in-process search entry and the `search` capability give the same answers for the same request.
- AC-17: waits and bulkhead (16 per plugin, 6 per tenant): two waiting submits from one tenant leave room for a `GET`, a `DELETE` and a stats call from that tenant, each answered within its deadline, and the same with a relayed waiting read over the in-process transport; a wait over 10 s is 400; a job past `max_running_time` is cancelled by the search's own deadline check and reads `FAILED` with `search_timeout`, with nobody reading it in between, and the search's cancel flag is read by the reader between its stages (cited in the slice 1 PR).
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

`serverless.async_search.enabled`, node scope, default off. Flip when VERIFIED.md records AC-13's counts, AC-8's peak and AC-15's comparison, and the principles' bar holds: no extra requests, no search regression with the plugin system on and the registry empty. The compatibility differences are listed as differences, not hidden by the flip. The limits are settings, not switches. The `job` capability is not behind a switch of its own: nothing grants it to a plugin whose policy does not ask.

## Risks

- A result is lost with its node and read through it; clients that expect stock's persistence (a poll after a restart, a long `keep_alive`) get 404. Documented in `compatibility.md`.
- The `job` capability touches the plugin contract and the ADR 0072 registry, both shared with other threads; slice 1 is kept to new files and a registry extension that leaves by-query tasks' tests unchanged.
- The `search` capability (plugin-system slice 4) may not exist when slice 2 starts; AC-16 holds the fallback and the capability to the same answers.
- The `Extensions` pool is 0% by default: until it is sized, every call is refused 429, which is why the plugin needs `serverless.plugins.enabled` and a sized pool.
- Stock shapes and the `keep_on_completion=false` after-completion rule are from documentation until recorded.
