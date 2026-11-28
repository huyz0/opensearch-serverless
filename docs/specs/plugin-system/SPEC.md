# Plugin system: native, WASM and gRPC tiers

## Serves

[ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md), the owner's request of 2026-11-28 for plugins in the manner of Envoy and Chrome. Builds on [extension-abi](../extension-abi/SPEC.md) and [extension-analyzer](../extension-analyzer/SPEC.md); changes none of their behaviour.

## Change

Seven slices, one PR each, each behind `serverless.plugins.enabled` (default off). Slices 1 and 2 add no tier a deployer can reach; the first plugin that runs is slice 2's native one.

1. **Contract** (`oss-ext`, no runtime): the manifest and its strict parser (the descriptor parser grown by the `plugin`, `routes`, `events`, `capabilities`, `budget` sections), the point catalogue as data (name, version, determinism class, ceilings, allowed tiers), the capability and budget types, the plugin API version (`1.0`) and its accept rule, the `rest` and `lifecycle` codecs, and the circuit breaker with an injected clock. Golden vectors for each codec.
2. **Registry, native tier, REST routes, lifecycle** (`oss-node` `plugins/`, `plugins/` for third-party native crates): the `Plugin` trait, the policy reader (`OSS_PLUGINS`), registration with collision and claim rules, route mounting with the action-table row, dispatch with `catch_unwind`, the breaker and bulkhead on every call, lifecycle delivery from a bounded queue, the status route, counters, the setting. A reference native plugin in `plugins/echo` used by the tests.
3. **WASM plugins:** the `rest` and `lifecycle` points on ADR 0052's host (an effectful point is allowed host imports, linked only when granted; a pure point still refuses any import), deadline accounting across host imports, the module's manifest, the load-time grant check.
4. **State, search and egress capabilities** for native and WASM: the `state` host API over document sets, the call budget and its 429, the `search` and `egress` imports with alerting's egress rules, tenant attribution of every request, rows in [request-budgets](../../request-budgets.md).
5. **gRPC tier** (`oss-ext-grpc`, feature `plugins-grpc`): the client, the `Describe` and `Call` messages, unix socket and mutual TLS, deadline, message bound reserved before read, connection reuse, breaker, `serverless.plugins.grpc.enabled`. A test server in Rust in `testkit` stands in for a plugin.
6. **Conformance, hostile soak, measurement, docs:** one suite run against all three tiers (the same plugin behaviour written three ways), hostile plugins per tier, the idle-zero and empty-registry measurements, `compatibility.md` (the status route, plugin routes, and that `_cat/plugins` does not list these plugins), the update of [add-an-extension-point](../../how-to/add-an-extension-point.md), the flip decision recorded.
7. **First adopter:** learning to rank's routes and rescore register through the system with its existing tests unchanged (the reference port other ports follow). Optional for the others; none is scheduled.

Not here: the other points (`ingest`, `query`, `pipeline_agg`, `script`, collecting `agg`), each wired by its own spec; a `schedule` capability; dynamic enablement; signed plugins; supervising out-of-process plugins; run-time native loading; any edit to the in-progress native ports.

File ownership: new files under `crates/ext`, `crates/ext-wasm`, `crates/ext-grpc`, `crates/node/src/plugins/`, `plugins/`, `docs/specs/plugin-system/`, plus an assembly hook in `crates/node` and the generated tables. Nothing in the files of index state management, alerting, anomaly detection or ML Commons.

## Acceptance criteria

- AC-1: the manifest is parsed by one strict parser whatever the tier delivers it: unknown, duplicate and mistyped keys are refused naming the key, depth, size and every length are bounded before allocation, a plugin id outside `[a-z][a-z0-9_-]{1,40}` is refused, and the same bytes give the same answer from a WASM descriptor, a `Describe` answer and a native `manifest()`.
- AC-2: the plugin API version rule is the ABI's: the same major and a minor at most the host's is accepted, another major or a newer minor is refused before anything else is called; golden fixtures built against `1.0` (a recorded WASM module and a recorded gRPC transcript) are accepted, and a change that breaks one fails a test.
- AC-3: the point catalogue is data and is enforced at load: each (point, tier) pair the catalogue forbids (`analyzer` on `grpc`, `script` on `grpc`, collecting `agg` on `wasm` or `grpc`) is refused naming both, and each pair it allows loads. A `pure` point with any host import or any capability is refused.
- AC-4: capabilities are granted by the node: a manifest asking for a capability the policy does not grant is refused at load and the plugin is `refused` in the status route; a granted capability the manifest did not ask for is not linked; a WASM module importing a host function it was not granted fails to load, not at call time; `grpc` plugins get no host capability and a manifest asking for one is refused.
- AC-5: routes: a plugin route is served with the method, path parameters, query, selected headers and body the `rest` codec carries, and answers with the plugin's status, headers and body within the point's ceilings. A plugin cannot mount a path outside `/_plugins/<id>/` without a `claim` in the policy; a route that collides with a built-in route is refused at load naming both; precedence is built-in, then plugin, then the proxy prefixes of ADR 0046. Authorization, `Authorization` and cookie headers are never passed, and `identity` passes the user name and roles only when granted.
- AC-6: a plugin route needs `all_access` unless the policy maps it to an action, and the action-table completeness test covers every mounted route (`crates/node/tests/server/security_authz.rs`).
- AC-7: native failures are one call, and no profile in any `Cargo.toml` sets `panic = "abort"` (a predicate in `scripts/check-layering.py` with a planted violation): a plugin that panics answers 500 `plugin_failed`, the node keeps serving the next request on the same worker, the counter and the status route show it, and no node lock is held across a plugin call, asserted by running the next request after a panic inside a call made while the dispatch edge's own state is in use.
- AC-8: WASM failures are one call: modules that spin, grow memory, trap, recurse, return an oversized or malformed output, or call a host import past its budget each answer their typed status ([ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md) failure table) and the next call is served, on the `rest` point as ADR 0052's hostile tests are on the test point.
- AC-9: gRPC failures are one call: against a test server that answers, is slow, hangs, drops the connection, answers a malformed message, answers more than the ceiling, answers a plugin 4xx and is absent, the host answers 200, 504, 504, 503, 502, 502, 400 and 503, never waits past the call's deadline, never holds a response buffer it did not reserve first, and the node keeps serving.
- AC-10: the circuit breaker is exactly the ADR's state machine under an injected clock: closed to open at 5 consecutive counted failures, or at half of the last 20 once 10 are seen; open for 30 s with 20% jitter; half-open admits one probe at a time; a probe's success closes it and its failure reopens it with the time doubled to 5 minutes. A 4xx and a 429 never count. While open a call makes **no call to the plugin and no store request**, and the answer is 503 at once.
- AC-11: the bulkhead holds: at most 8 calls to a plugin at once and 2 for one tenant, the ninth and the third answer 429 immediately and nothing queues, for each tier.
- AC-12: **an idle plugin makes no request** (and the gRPC runtime starts at the first connect, spawns no timer, has h2 keepalive off, and a connection dropped while idle is reopened only by the next call, not by the host): with a native, a WASM and a gRPC plugin loaded and an hour of mocked time with no call, the request meter reads exactly the startup reads (1 GET a WASM module per node) and nothing else, and the gRPC test server sees one `Describe` and no other call; no timer or health poll is started for any plugin.
- AC-13: **state costs what the table says**: `get` cold is 2 GET at depth 2, warm 0; `put` is at most 1 GET and 2 PUT; `delete` 1 GET and 1 PUT; a lost swap adds one more attempt up to 3 then 409; the meter shows no LIST for any of them; every request is attributed to the calling tenant and the plugin id. Each count is a row in [request-budgets](../../request-budgets.md) that the test pins.
- AC-14: **a call has a budget**, in the units of requests the tests count (a `searches` unit is one search whatever its requests), and a plugin's call that enters a plugin route or point is 400 `plugin_recursion`; also a put loop past `state_ops_per_second` is 429: with a manifest budget of 2 state operations, the third in one call is 429 `plugin_budget_exceeded` and costs no request; the default budget is zero of each; the policy's ceiling lowers a manifest's budget; a `search` or `egress` over its budget is refused the same way.
- AC-23: egress is safe: a target resolving to a private, loopback or link-local address, a redirect, a DNS answer that changes between resolve and connect, a body over 1 MiB and an unverified certificate are each refused, with a planted target for each.
- AC-24: state is bounded: a put past 1,000 keys or 4 MiB for one (plugin, tenant) is 413, content orphaned by a lost swap is at most one blob per lost swap, and the named collector removes it.
- AC-25: a tenant cannot open a breaker for others: one tenant's calls that trap open that tenant's breaker only, another tenant is served; connection refused opens the plugin's.
- AC-15: isolation of state: a key outside the plugin's set cannot be named (`..`, a separator, an overlong key, another plugin's id are refused), a call for tenant A never reads or writes tenant B's set, and a plugin that is not granted `state.*` has no state API at all.
- AC-16: memory: a plugin route's request body is reserved from the `Extensions` pool before it is read and its response ceiling before the call, so a body larger than `max_request_bytes` is a 413 before anything is read and an exhausted pool is a 429; the peak reservation of each tier equals the literal sum the ADR states in section 7 (native `body + response ceiling + ctx reservations`, wasm ADR 0052's formula plus body and response, grpc `body + response ceiling + 64 KiB window and frame pair per connection`) and is held to it by a test; a native plugin's `ctx.reserve` refusal is a 429; with the pool at 0 no WASM or gRPC plugin loads and the start log says so.
- AC-17: lifecycle events are delivered to a plugin in the order they happened, from a bounded queue of 256 with drop-newest and a counter; a plugin that blocks forever delays no index create, delete or node start (the operation completes within its normal time with the plugin stuck); an event is delivered with no request to the store.
- AC-18: the status route answers from memory with id, tier, version, state, counters and last failure, makes no store request, and requires `all_access`; the counters are in `/_prometheus/metrics`.
- AC-19: the switch (the numbers are provisional, measured on RustFS): with `serverless.plugins.enabled` off nothing loads, a policy naming plugins is logged and ignored, plugin routes answer as unknown routes, the status route is absent, and every existing test (proxy prefixes, ported routes, the analyzer pin and its switch) passes unchanged; with it on and an empty policy, search and write latency and the request counts of the budget tests are unchanged (within the bar of the build rule: write p99 within 5%, no extra requests, no search regression).
- AC-20: the layering predicate: a crate under `plugins/` that depends on any workspace crate other than `oss-ext` fails `scripts/check-layering.py`, with a planted violation in `scripts/selftest-checks.py`; a crate outside `plugins/` and `oss-node` that depends on `oss-ext-grpc` fails it too.
- AC-21: conformance: one suite whose plugin is written natively, as a WASM module and as a gRPC service gives the same answers for the same requests for `rest` and `lifecycle`, including every status in the failure table, on all three tiers.
- AC-22: the first adopter: learning to rank's routes and rescore, registered through the system, pass their existing tests unchanged and make the same store requests as before (`docs/specs/ltr`).

## Test plan

- AC-1, 2, 3: unit and golden tests in `crates/ext/tests/`, each seen red against a parser that accepts what the criterion refuses; the golden module and transcript are checked in.
- AC-4, 5, 6: `crates/node/tests/plugins/` with the `plugins/echo` native plugin and a table-driven policy; the collision and claim cases are one test per refusal. Catches a plugin shadowing a built-in or reaching a capability it was not granted.
- AC-7: a native plugin whose handler panics; asserts the next request on the same server succeeds and the registry, breaker and bulkhead are still usable. Catches a panic that kills a worker or leaves dispatch state poisoned.
- AC-8: hostile modules built from WAT in `crates/ext-wasm/tests/` for the `rest` point, one per limit, the way `hostile.rs` is for the test point.
- AC-9: a gRPC test server in `testkit` with one misbehaviour a test; a deadline test uses a hang and asserts the elapsed time is within the deadline plus a bound. Catches a client that waits on a hung peer or reads a response of the peer's choosing.
- AC-10: the breaker is pure, so a table of event sequences against expected states with a fake clock; a second test asserts through the request meter and a call counter that an open breaker makes zero of either.
- AC-12: the request meter and a virtual clock over an hour, as `virtual-time-soak` does; a mutation adding a poll fails it.
- AC-13, 14, 15: the request meter on the in-process emulator; the counts are read from the budget table, not repeated in the test.
- AC-16: the pool's own accounting under a gate that holds calls, as `hostile.rs` does for the formula.
- AC-17: a plugin whose lifecycle handler never returns; asserts index create latency against a baseline.
- AC-19: the full existing suite with the switch off, and the budget tests with it on and empty.
- AC-20: `scripts/selftest-checks.py` planted violation, red first.
- AC-21: one parameterised suite, three plugin builds.

## S3 request budget

Per operation, worst case; the numbers are the ADR's table and each is a row in [request-budgets](../../request-budgets.md) with its enforcing test.

| Operation | PUT-class | GET-class | LIST | Sequential depth |
|---|---|---|---|---|
| Idle plugin, any tier | 0 | 0 | 0 | 0 |
| Load a native plugin, load a gRPC plugin | 0 | 0 | 0 | 0 (one `Describe` call to the process) |
| Load a WASM plugin | 0 | 1 per node per module, none per call | 0 | 1 |
| A `rest` call with a zero budget | 0 | 0 | 0 | 0 |
| `state.get` cold / warm | 0 | 2 / 0 | 0 | 2 / 0 |
| `state.put` | 2 | at most 1 | 0 | 2 |
| `state.delete` | 1 | 1 | 0 | 2 |
| A lifecycle event, the status route, an open breaker | 0 | 0 | 0 | 0 |
| `search` capability | as the search | as the search | 0 | as the search |
| `egress` capability | 0 | 0 | 0 | 0 (an HTTP call, not a store request) |

No request scales with documents, indexes, plugins loaded or elapsed time. A call's requests are at most its budget, which is declared, capped and counted.

## Switch

`serverless.plugins.enabled`, node scope, default **false**; `serverless.plugins.grpc.enabled`, default false, only with the first. Both flip only when slice 6 records: hostile plugins of every tier leave the node serving within its memory budget; AC-12's idle-zero holds with all tiers loaded; an enabled empty registry changes no request count and keeps write p99 within 5% and search latency unchanged (measured on RustFS, provisional). Until then the setting stays off, the flip is its own change, and the owner is told.

## Risks

- **A native plugin is the node's own fault domain.** The matrix keeps untrusted code out of it; a hang or a stack overflow in a native plugin is the node's. The status route and the breaker limit blast radius after the fact, not before.
- **Three transports can drift.** AC-21's suite and the golden fixtures are the answer; if one tier needs a rule the others lack, the point's spec says so.
- **A synchronous host import inside a WASM call blocks the caller's thread for a store read.** The deadline covers it and the call budget bounds the count; measured in slice 6 before any default moves.
- **gRPC adds an outbound connection surface.** Off by default, endpoints named by the deployer in the policy, no redirects, no discovery; TLS verification is not optional for TCP.
- **The policy file must match across nodes.** A node with a different policy answers differently; the status route makes it visible, and a mixed policy is a deployment error this does not detect (the same as ADR 0052's node switch).
- **ADR number.** 0080 leaves room for the numbers other threads have taken; renumber before merge if one is taken.
