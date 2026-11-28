# 0080: One plugin system, three tiers: native in-process, sandboxed WASM, and out-of-process gRPC, behind one manifest, one point contract and one failure rule

**Status:** Accepted after independent review (one round, accept with changes, all folded in: tokio availability, the panic-abort predicate, per-tenant breaker, state caps, search recursion, egress rules, memory formulas), 2026-11-28

## Context

Today a Rust node extends in four unrelated ways:

- **Compiled-in ports.** Learning to rank ([ADR 0071](0071-learning-to-rank-is-ported-natively.md)), alerting ([ADR 0075](0075-alerting-is-ported-natively.md)) and the ports in progress (index state management, anomaly detection, ML Commons) each register their own routes, settings and background work in node assembly by hand. Nothing says what they may touch, and one panic in one of them is the node's panic.
- **A reverse proxy** for stock Java plugins that are not ported ([ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md)).
- **WASM extension modules** on a bytes-in, bytes-out ABI ([ADR 0052](0052-extension-modules-on-a-wasm-abi.md)), with one wired point, `analyzer` ([ADR 0053](0053-the-analyzer-extension-point.md)). The runtime is general; its only caller is an analyzer pin.
- A way to add a REST route, a lifecycle hook, an ingest step or a query type from outside the tree does not exist.

The owner asked (2026-11-28) for a real plugin system in the manner of Envoy and Chrome: in-process plugins, WASM plugins, and out-of-process plugins over gRPC. Envoy's answer is three tiers with one filter contract: static extensions compiled in, Wasm filters in a sandbox with fuel-like limits, and external services (`ext_authz`, `ext_proc`) over gRPC with deadlines and a failure mode chosen per filter. Chrome's is built-in components, a sandboxed renderer, and separate utility processes behind IPC with a broker that grants capabilities. What carries over is the shape: **one contract, several isolation levels, capabilities granted by the host and never taken, and a failure that is a typed answer to one call.**

Forces, from the owner's principles and the repository's rules:

- **S3 requests are the bill.** Requests scale with nodes and bytes, never with records, documents, indexes or time per index; idle indices make zero requests; no LIST on a hot path. A plugin must not be a way around that: every store access goes through a metered, budgeted host API, with a worst case stated per operation.
- **Memory is reserved before it is held** ([ADR 0004](0004-bounded-memory-and-streaming.md)): every request body, response body and instance a plugin can hold is a reservation.
- **No lock, lease or leader election for correctness**, and nodes own nothing. A plugin cannot take a lock the node depends on, and a plugin's state is blobs written by compare-and-swap.
- **Scalability** (hundreds of nodes, ten million tenants): the cost of a plugin is per call and per tenant, never per node pair or per index.
- A **stable, versioned** surface, because plugins are built outside the tree and outlive a node release.
- `unsafe` is allowed only the guaranteed-safe way ([ADR 0009](0009-unsafe-when-guaranteed-safe.md)); loading a native library at run time is not one of those ways.
- A feature with a measured downside is a switch ([principles](../principles.md#switches-and-defaults)).

Four ports are in flight in other threads. Their files are not touched here; the design below says how each could move onto it later.

## Decision

### 1. Three tiers, one plugin

A **plugin** is an id (`[a-z][a-z0-9_-]{1,40}`), a version, a **manifest**, and an implementation in exactly one tier. The tier is the isolation level, chosen by whoever deploys it:

| Tier | What it is | Trust | Isolation | Latency (design, unmeasured) |
|---|---|---|---|---|
| `native` | Rust compiled into the node binary through a Cargo feature, registering through the `Plugin` trait | trusted, reviewed like the node | none: the node's own fault domain; a panic is caught at the dispatch edge | a function call |
| `wasm` | A module of [ADR 0052](0052-extension-modules-on-a-wasm-abi.md), content-addressed in the store | untrusted | the sandbox: fuel, linear memory, deadline, stack, concurrency gate, reservation | a call into an instance, microseconds to a millisecond |
| `grpc` | A separate process, any language, reached over gRPC on a Unix socket or mutual-TLS TCP | untrusted | the operating system's process boundary, plus deadline, concurrency bulkhead, message bound, circuit breaker | a round trip, about a millisecond on loopback |

There is **no run-time loading of native code** (no `dlopen`, no shared library): it would need `unsafe`, an unstable Rust ABI, and it would put untrusted code in the node's address space. Code that is not trusted enough to compile in is WASM or a process. This is the line Envoy and Chrome draw: static extensions are the trusted tier.

The node **does not start, supervise or restart** out-of-process plugins. A sidecar, a systemd unit or a pod owns the process, as for Envoy's external services. The node connects, calls, and fails the call when the process is away.

### 2. One manifest, judged the same whatever loads it

The manifest is the descriptor of ADR 0052 grown by one section, parsed by the same strict parser in `oss-ext` (unknown and duplicate keys refused, depth and size bounded, every length checked before use). It is the module's `oss_describe` output for `wasm`, the answer to the `Describe` call for `grpc`, and `Plugin::manifest()` for `native`.

```
{ "plugin": { "id": "geo", "version": "1.2.0", "api": "1.0" },
  "points": [ {"name":"rest","version":1}, {"name":"lifecycle","version":1} ],
  "routes": [ {"method":"GET","path":"/_plugins/geo/{place}","action":"read"} ],
  "events": ["index.created","index.deleted"],
  "capabilities": ["state.read","state.write","egress:api.example.com"],
  "budget": { "state_ops": 2, "searches": 0, "egress": 1 },
  "limits": { "fuel": 20000000, "memory_bytes": 16777216, "deadline_ms": 2000 } }
```

A manifest is a **request**. What a plugin gets is the intersection with the node's **grant** (section 4); a manifest that asks for more than its grant is refused at load, not trimmed (a plugin that silently runs without a capability it needs fails later and elsewhere). `limits` may only lower the host's, as in ADR 0052. A plugin is loaded before it serves, never at call time.

### 3. One point contract, three transports

A **point** is a name, a version, and a wire encoding for a block in and a block out, with ceilings per point and a **determinism class**:

- `pure`: the output is a function of the input alone, no host import, no clock. Output becomes durable state (terms in a segment) or must be replayed identically. `analyzer` is pure ([ADR 0053](0053-the-analyzer-extension-point.md)).
- `effectful`: may use granted capabilities and the clock; never replayed.

The contract is defined once, in `oss-ext`, as a bytes codec with strict validation of every output; the host validates the output of every tier before use, including `native` (a plugin's output is data to the node, not code). Three adapters carry the same bytes: WASM's `oss_call(point, in_len, out_cap)`, gRPC's `Call`, and a direct trait call for native. Status codes are the ABI's. A point's version is matched exactly; a new point or a new optional field is additive.

**Point catalogue and which tier may serve it.** A manifest naming a point on a tier its row forbids is refused at load.

| Point | Does | native | wasm | grpc | Why |
|---|---|---|---|---|---|
| `rest` | serves routes the plugin declares | yes | yes | yes | one call per request, bytes in and out |
| `lifecycle` | observes node and index events; cannot veto | yes | yes | yes | delivered async from a bounded queue |
| `analyzer` | named analyzer pinned at index create | yes | yes | **no** | durable terms and replay need the module to be part of the index; a network service on the recovery path is not ([ADR 0053](0053-the-analyzer-extension-point.md)) |
| `ingest` | ingest-pipeline processor over a block of documents | yes | yes | yes, fails closed | the log holds the result, so replay never re-runs it; a down service refuses the write |
| `query` | a query type that rewrites to built-in queries | yes | yes | yes | one call per query at search time; the output is a built-in query tree the reader already validates |
| `pipeline_agg` | a pipeline aggregation over reduced buckets | yes | yes | yes | one call per request over bounded buckets |
| `script` | a scoring or field script over a block of documents | yes | yes | **no** | per-document work cannot afford a round trip |
| `agg` (collecting) | a new aggregation that collects over documents | yes | **no** | **no** | per-document callbacks; native only |

Only `rest` and `lifecycle` are built by this ADR's slices. Every other point is wired later, one point at a time, by [How to add an extension point](../how-to/add-an-extension-point.md) (its own spec, its own sites, its own checks), exactly as `analyzer` was; the catalogue fixes which tiers each may use so nobody has to decide it again.

**Versioning.** The plugin API is `1.0`, `major.minor`, accepted by the rule of `oss_ext::abi_accepted` (same major, a minor at most the host's). Additive changes (a point, an optional manifest key, a host import with a new versioned name such as `state_get_v1`) bump the minor; a break bumps the major, and the host serves the previous major for at least two releases, announcing it at start. A recorded WASM module and a recorded gRPC transcript built against `1.0` are golden fixtures that every later host must still accept.

**This amends ADR 0052's import rule.** A module there imports only `log` and `cancelled` and `validate.rs` refuses any other import. An `effectful` point may import the capability functions the policy granted, by versioned name; `pure` points keep ADR 0052's rule unchanged.

### 4. Capabilities: the host grants, the plugin asks, nothing is ambient

A node's **plugin policy** (a JSON file named by `OSS_PLUGINS`, read at start, the same on every node; dynamic reload is later and the route table is built behind a swappable pointer so it can be added) lists, per plugin id: the tier and where it is (native: the Cargo feature name; wasm: the module's SHA-256; grpc: the endpoint and, for TCP, the client certificate and the server name to verify), the **grants**, and ceilings for budget and limits. The manifest asks; the policy grants; the effective set is what is both, and an over-ask refuses the load.

Capabilities, v1:

| Capability | Gives | Tiers | Cost |
|---|---|---|---|
| `state.read`, `state.write` | tenant-scoped key-value state under `plugins/<id>/<tenant>/`, section 5 | native, wasm | the table in section 5 |
| `search` | run a search as the calling tenant through the reader | native, wasm | a search; charged to the tenant |
| `egress:<host>` | bounded HTTP(S) to an allow-listed host | native, wasm | no store request; resolve once, check the resolved address against private, loopback and link-local ranges (refused unless the node's policy allows it), connect to that address (no DNS rebinding), no redirect, body at most 1 MiB, time at most the call's deadline, TLS verified |
| `identity` | the caller's user name and roles in the request | all | none |
| `log`, `metrics` | host-rate-limited log lines and plugin-labelled counters | native, wasm | none |

A plugin's call that makes a `search` cannot enter a plugin route or point: the depth is 1 and a nested entry is refused, so a plugin cannot recurse through the reader. A `searches` unit in a budget is one search with its own cost (a cold index is 3 GET at depth 3 by [request-budgets](../request-budgets.md)), so the policy's ceiling is stated in searches and the tests count requests.

Never granted: a blob store handle, a LIST, a lock, a thread, a file, a socket, the clock to a `pure` point.

**`grpc` plugins get no host capabilities** (only `identity`, which is data in the request, not a call back). They are services with their own storage, as Envoy's external services are. One that wants this cluster's data calls the node's public API as any client does, under the security layer and request meter ([ADR 0051](0051-authorization-at-the-route-by-action-table.md)). The host never calls out of the sandbox on a plugin's behalf, so the cost of its state is not the node's, and nothing the node holds is reachable through the process boundary.

**Routes.** A `rest` route is mounted only if the policy grants it. A plugin owns `/_plugins/<its id>/...` by default; any other path (a ported plugin's stock prefix, say) needs an explicit `claim` in the policy. Precedence is built-in routes, then plugin routes, then the proxy prefixes of ADR 0046, so a plugin can never shadow a built-in and a ported plugin leaves the proxy list as it does today. A collision is a refusal at load, naming both. Every plugin route is `all_access` in the action table of ADR 0051 unless the policy maps it to an action; the row is added at load, and the existing completeness test covers it.

### 5. State and S3 cost: a host API with a stated worst case, and a budget per call

A plugin's state is a **document set** under `plugins/<id>/<tenant>/` (a marker register naming immutable content blobs, the shape of stored scripts and learning to rank), reached only through the host's `state` API: `get(key)`, `put(key, bytes, if_version)`, `delete(key, if_version)`. **There is no list**: keys are derived by the plugin, as the node's own keys are. A key is at most 128 bytes, a value at most 256 KiB, a set at most 1,000 keys and 4 MiB in all per (plugin, tenant) (`serverless.plugins.state_max_keys`, `state_max_bytes`; a put past either is 413), so plugin state is a bounded storage cost per tenant; a put loop is bounded per call by the budget and per time by `serverless.plugins.state_ops_per_second` per (plugin, tenant) (default 20, 429 past it). Content written by a swap that lost is orphaned at most once per lost swap and is named by the key's hash so a retry rewrites the same blob; the slice names its collector in the spec, as the document-set stores of learning to rank and stored scripts do, and never lists on a hot path; a tenant sees only its own set (the tenant is the host's, from the authenticated request, never a plugin argument), and a plugin id is a path component the parser has already restricted.

Worst case per operation, uncontended (the rows go in [request-budgets](../request-budgets.md) and a test pins each):

| Operation | Requests | Round trips |
|---|---|---|
| `get`, cold | 2 GET (the set's marker, then the content) | 2 |
| `get`, warm (inside `serverless.plugins.state_cache_ttl`, default 30 s) | 0 | 0 |
| `put` | 1 GET (marker, unless its etag is held) + 2 PUT (content, marker swap) | 2 |
| `delete` | 1 GET + 1 PUT | 2 |
| a lost swap | one more of the above, at most 3 attempts, then 409 | |
| lifecycle event, status route, a load of a native or gRPC plugin | 0 | 0 |
| load of a WASM plugin | 1 GET of the module per node ([ADR 0052](0052-extension-modules-on-a-wasm-abi.md)) | 1 |

**Every call has a budget.** The manifest declares the most state operations, searches and egress calls one call may make (default **zero** of each), the policy caps it, and the host counts and answers 429 `plugin_budget_exceeded` to the next one, so a plugin's cost per request is bounded by a number a reviewer can read, and that number multiplied by the request rate is what the plugin can cost, never a function of how many documents, indexes or days there are. Every operation is charged to the calling tenant's request counters, labelled with the plugin id. **Idle plugins make zero requests**: the host starts no timer, no health check and no poll for any plugin, a lifecycle event is delivered only because the event happened, and a breaker probes only when a call arrives. There is no background-job capability in v1; the alerting runner's slot-claim protocol is the model for a later `schedule` capability (section 9), not something a plugin can do on its own today.

A native plugin that uses the host API gets the same counting and the same numbers. The in-tree ports, which use the store layers directly under their own specs, are first-party and stay under those specs; **third-party native plugins live in crates under `plugins/` that may depend on `oss-ext` and nothing below it** (no `oss-blob`, no `oss-meta`), which `scripts/check-layering.py` enforces as a predicate over `Cargo.toml` files, so the only store access such a plugin has is the host API.

### 6. Failure: a plugin failure is the failure of one call

The rule across all tiers: **a plugin fails a call, never the node, and a failure that would make a stored thing wrong refuses instead of falling back.**

| What went wrong | Answer | Counts toward the breaker |
|---|---|---|
| plugin refuses the input deterministically (its own 4xx status) | 400 `plugin_failed`, the plugin's detail made printable and bounded | no |
| fuel, memory, deadline or stack limit; a trap; a panic (native: caught at the dispatch edge) | 400 or 500 `plugin_failed` naming the limit | yes |
| output malformed (the host's validation) | 502 `plugin_malformed` | yes |
| deadline passed (gRPC and host calls inside a wasm call included) | 504 `plugin_timeout` | yes |
| capacity: gate full, pool cannot hold the reservation, budget used | 429, nothing queued | no |
| a plugin's call that enters a plugin route or point (through `search`) | 400 `plugin_recursion`, depth is 1 | no |
| breaker open, process away, connection refused | 503 `plugin_unavailable` at once, no call made | the cause counted |

Where a plugin sits on a path that builds durable state (`analyzer`, `ingest`), its failure **refuses the operation** (503 or 4xx), never skips the plugin; where it only observes (`lifecycle`), the event is dropped and counted and the operation it observed is never delayed: delivery is from a bounded queue of 256 events a plugin, drop-newest, so a plugin that stops answering costs a counter, not a thread.

**Circuit breaker**, shared by all three tiers, pure logic in `oss-ext` with an injected clock. Failures a caller's input can cause (a trap, a limit, malformed output, a panic) count toward a **per-(plugin, tenant)** breaker (a table bounded at 1,024 entries, least recently used dropped), so one tenant's hostile input cannot open the breaker for others; failures that are the plugin's own (connection refused, transport timeout, process away) count toward the **per-plugin** breaker. Each: closed to open on 5 consecutive counted failures, or on at least half of the last 20 calls failing once 10 have been seen; open for 30 s with 20% jitter; then half-open, admitting one probe at a time; a success closes it, a failure reopens it with the time doubled up to 5 minutes. An open breaker answers 503 without calling, so a dead plugin costs nothing and cannot hold a thread. Per-plugin concurrency is bounded by a **bulkhead** (the gate of ADR 0052: 8 a plugin, at most 2 for one tenant), and a call's deadline is the least of the caller's remaining time, the point's ceiling and the plugin's limit.

Honest limits of `native`: it is the node's own code. A call that never returns holds its worker (the deadline there is checked and counted when it returns, and the breaker then opens); a stack overflow or an abort is the process's. That is why the matrix keeps untrusted code out of that tier and why a native plugin is a reviewed build, not an upload. A panic is caught with `catch_unwind` at the dispatch edge, which is sound only while no profile sets `panic = "abort"`: a predicate (`scripts/check-layering.py` reads every `Cargo.toml`) refuses it, with a planted violation. A native call that never returns leaks one worker per call until the plugin's bulkhead is full, which is a denial of service of that plugin alone (its calls answer 429, then the node's other routes are unaffected); this is stated, not hidden, and is the reason the tier is trusted. The dispatch edge takes no node lock across a plugin call, so a panic poisons nothing the node needs.

**Visibility.** `GET /_plugins/_serverless/plugins` answers from the node's memory with the policy's hash (so a rollout where nodes differ is visible, and a client may see a plugin route answer 404 on one node and 200 on another until the policy is the same everywhere), and each plugin's id, tier, version, state (`ready`, `breaker_open`, `refused`), counters and last failure; `all_access`, no store request, a difference from stock OpenSearch listed in [compatibility](../compatibility.md). The same counters are in `/_prometheus/metrics`.

### 7. Memory

All three tiers draw on the `Extensions` pool of ADR 0052 (`OSS_EXTENSIONS_PERCENT`, 0 to 20, default 0): a WASM call's reservation as there; for every call, the request body and the response ceiling before either is read (a response is read into a buffer the host reserved for the point's ceiling, never grown by the plugin's word); a native plugin's own working memory through `ctx.reserve`, a refusal being a 429; a gRPC connection's buffers once at connect. The peak a call holds is stated per tier: native, `request body + response ceiling + ctx reservations`; wasm, ADR 0052's `linear memory + 2 x io_bytes + stack` plus the body and response ceiling; grpc, `request body + response ceiling + the connection's buffers (one 64 KiB window and frame pair per connection)`. A request body for a plugin route is bounded by `serverless.plugins.max_request_bytes` (default 1 MiB, at most 10 MiB) and a response by the point's ceiling (default 4 MiB); nothing streams in v1. With the pool at 0, no wasm or grpc plugin loads and the node says so at start.

### 8. Switch, defaults and the build rule

- `serverless.plugins.enabled` (node setting, default **false**): with it off nothing loads, no plugin route exists, the status route is absent, and a policy naming plugins is logged and ignored. It stays off until the soak of the spec shows hostile plugins of every tier leave the node serving within its budget, an idle node with plugins loaded makes zero requests, and an enabled-but-empty registry costs nothing measurable on search and write latency. Loading code a deployer chose is a risk a switch exists for; an empty registry's cost is measured, not assumed.
- `serverless.plugins.grpc.enabled` (default false, requires the above): the one tier that opens outbound connections.
- Existing tests pin the old behaviour: the proxy, the ported routes, the analyzer pin and its switch are unchanged. New tests cover the new behaviour; both run in CI.
- A native plugin compiled in is dormant with the setting off: it registers nothing, mounts no route and receives no event, so a build with a plugin feature behaves as one without it. Existing in-tree ports keep their own switches and are not governed by this one until they adopt the system.

### 9. How the native ports could move onto it

None must. A port moves in its own change, by its owning thread, when it is stable, by wrapping what it already has: its `routes()` becomes a `rest` registration, its settings stay in its settings registry, and an index-created or index-deleted hook it wants becomes a `lifecycle` subscription instead of an edit in node assembly. What each gains is a uniform status entry, a panic edge, a breaker, and the plugin id on its meter lines; what it keeps is its own store layers and request budgets under its own spec, as first-party code.

- **Index state management:** `rest` plus `lifecycle` (index created, to attach a policy). Its state machine needs a ticking runner.
- **Alerting:** `rest` plus its runner. Its slot-claim protocol (derive the next slot from the schedule and the clock, claim it by one compare-and-swap, the top two placed nodes only, at most once a slot) is the design a future `schedule` capability would lift into the host so a plugin cannot invent a poll; that capability is a minor-version addition when a second consumer exists.
- **Anomaly detection:** `rest`, a runner, and `search`.
- **ML Commons:** `rest`, and its remote-model connectors are the natural first out-of-process (`grpc`) users, which keeps model credentials in the plugin's process.
- **Learning to rank**, already merged, is the reference adopter in the last slice: its routes and `sltr` rescore registered through the system with its existing tests unchanged.

This work edits none of those ports' files. New code is in `crates/ext` (contract, breaker), `crates/ext-wasm`, a new `crates/ext-grpc`, `crates/node/src/plugins/` and `plugins/`; the only edits to shared files are an assembly hook in `crates/node` and rows in generated tables.

### 10. Crates

`oss-ext` grows the manifest, the point contract, the capability and budget types and the breaker (still no runtime, no workspace dependency). `oss-ext-wasm` gains the effectful host imports, linked only when granted. A new `oss-ext-grpc` holds the client (`h2` and `prost` for the small message pair, on a runtime the crate owns: tokio and `h2` are in the tree through `oss-edge`, behind its `edge` feature, and the node itself has no async executor, so `oss-ext-grpc` creates a small fixed-size runtime lazily at the first connect, with h2 keepalive disabled and no timers, exposes only a blocking `call(deadline)`, and states its thread count), built only with a node Cargo feature `plugins-grpc`, so a build without it has no new dependency. The registry, dispatcher, policy reader, state host API and the routes live in `oss-node`'s `plugins/` module. The layering check and port map get entries for each.

## Alternatives rejected

- **Native shared libraries loaded at run time.** Needs `unsafe` and a stable ABI Rust does not have, and puts untrusted code in the node's address space. Rejected; compile-in is the trusted tier.
- **gRPC only, with WASM and native as special cases of it.** Costs a round trip on every in-tree extension and puts the analyzer's recovery path on a network.
- **WASM only.** The owner asked for three, and some things (a model server, a service with its own database, a language with no WASM target) belong in a process.
- **Plugins reaching the blob store through the handle the node has.** Makes every plugin's cost invisible to the budgets; the host API is the point.
- **Host capabilities offered to gRPC plugins through a callback channel.** Lets an out-of-process plugin use the node's authority; as an API client it uses its own credentials under the security layer.
- **A background-job capability now.** The only way to get it wrong is an elapsed-time cost per index; alerting's protocol is the model and is built first by its own port.
- **Supervising out-of-process plugins in the node.** Process management is a deployment concern and a large new surface for the node.
- **Dynamic enablement at run time.** A route table that changes under load needs plumbing nothing else needs; start with the policy at boot.

## Consequences

- One manifest, one contract and one failure rule across three tiers, so a plugin can move tier without changing what it is; the catalogue fixes where each point may run.
- The cost of a plugin is a number in its manifest times its request rate, enforced by the host and attributed to the tenant.
- A bad WASM or gRPC plugin cannot take the node down, hold its memory past a reservation, or cost requests beyond its budget; a bad native plugin can, and the design says so rather than pretending otherwise.
- Harder: three transports to keep in step (golden vectors and one conformance suite run against all three), a larger build matrix (a `plugins-grpc` feature in CI), and a policy file deployers must keep identical across nodes.
- Rules out: run-time native loading, process supervision by the node, ambient capabilities, state listing, a plugin-owned timer.
- Not decided here, each its own ADR: each non-REST point's wiring, a `schedule` capability, dynamic enablement, signed plugins, resource limits for native plugins beyond reservation.
- Acceptance criteria and slices are in [docs/specs/plugin-system/SPEC.md](../specs/plugin-system/SPEC.md).
