# 0080: One plugin system, three tiers: native shared libraries loaded at run time, sandboxed WASM, and out-of-process gRPC, behind one manifest, one point contract and one failure rule

**Status:** Accepted, 2026-11-28. First text accepted after one independent review round. The amendment (the native tier as a dynamic library) had a second round, accept with changes, all folded in; the exception to ADR 0009 in section 1a was accepted by the owner on 2026-11-28.

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
- `unsafe` is allowed only the guaranteed-safe way ([ADR 0009](0009-unsafe-when-guaranteed-safe.md)). Loading a shared library runs code the compiler never saw, so it cannot meet that bar by construction; section 1a states what stands in for it and asks the owner to accept the exception.
- A feature with a measured downside is a switch ([principles](../principles.md#switches-and-defaults)).

Four ports are in flight in other threads. Their files are not touched here; the design below says how each could move onto it later.

## Decision

### 1. Three tiers, one plugin

A **plugin** is an id (`[a-z][a-z0-9_-]{1,40}`), a version, a **manifest**, and an implementation in exactly one tier. The tier is the isolation level, chosen by whoever deploys it:

| Tier | What it is | Trust | Isolation | Latency (design, unmeasured) |
|---|---|---|---|---|
| `native` | A shared library (`.so`, `.dylib`) loaded when the node starts, speaking a stable C ABI; also linkable into the node for first-party code, through the same trait | trusted: the operator allow-lists its SHA-256 or its signing key | none: the node's own address space and fault domain; a watchdog, a breaker and a crash-loop guard limit what a fault costs | a function call through a table |
| `wasm` | A module of [ADR 0052](0052-extension-modules-on-a-wasm-abi.md), content-addressed in the store | untrusted | the sandbox: fuel, linear memory, deadline, stack, concurrency gate, reservation | a call into an instance, microseconds to a millisecond |
| `grpc` | A separate process, any language, reached over gRPC on a Unix socket or mutual-TLS TCP | untrusted | the operating system's process boundary, plus deadline, concurrency bulkhead, message bound, circuit breaker | a round trip, about a millisecond on loopback |

The native tier is **a dynamic plugin, as in Envoy**: a library built against the plugin SDK (`oss-plugin-sdk`; for Rust a `cdylib`, for any other language anything that exports C symbols), loaded by the node from a path the policy names. There is no stable *Rust* ABI, so nothing but `#[repr(C)]` data and `extern "C"` functions crosses the boundary (section 1a). The same plugin may instead be linked into the node as a `NativeInstance` (one trait the loader also implements), which is how first-party code such as the ported plugins registers until it needs its own release cadence; both forms go through the same dispatch, breaker and status route. Code that is not trusted enough to be allow-listed is WASM or a process.

### 1a. The native tier in detail

**The ABI.** `oss-ext` defines the C ABI as `#[repr(C)]` types and symbol names (no `unsafe` there). The library exports `oss_plugin_abi() -> u32` (the packed plugin API, judged by the manifest's rule before anything else is called), `oss_plugin_manifest(out: *mut OssBuf) -> i32` (the manifest JSON, written into a buffer the host owns), `oss_plugin_init(host: *const OssHost, instance: *mut *mut c_void) -> i32`, `oss_plugin_call(instance, point, input: OssSlice, output: *mut OssBuf) -> i32` and `oss_plugin_shutdown(instance)`. **The output buffer is the host's**: an `OssBuf` carries a pointer, a `cap` (the point's ceiling, reserved before the call) and a `len` the plugin sets; the plugin writes into it, never hands the host memory it allocated, and a returned `len` above `cap` is `plugin_malformed`. An `OssSlice` and an `OssBuf` are valid for the duration of the call only; the plugin copies what it keeps. The same holds toward the plugin: **the host never returns memory it allocated**; a table function such as `state_get` fills a buffer the plugin provides and reports the length it needed. `OssHost` is a table of function pointers that starts with its own `size` and `version` and is append-only: a plugin built against an older host sees only the fields it knows, and one built for a newer host sees `size` too small for what it needs and refuses to start. Every table function is `extern "C"`, takes the call context, and is wrapped in `catch_unwind` on the host side, so a host panic never unwinds into the plugin. A capability the policy did not grant is a stub that answers `denied`, never a null pointer. `oss_plugin_call` is invoked from many threads at once, up to the bulkhead of 8; the manifest declares `"threading": "concurrent"` (the plugin is thread-safe) or `"serial"` (the host serialises calls to it), and a manifest that omits it is refused. The manifest also declares `"panic": "unwind"`, because the SDK's `catch_unwind` needs it, and a plugin that declares `abort` is refused. The status codes are the ABI's. A panic must not unwind across the boundary: the SDK wraps every export in `catch_unwind` and turns a panic into the module-failure status; a plugin built without the SDK that unwinds into the host aborts the node, which section 6 treats as a crash. Version negotiation is the host's one rule (same major, a minor at most the host's) in both directions: the plugin checks `OssHost.version` and the host checks `oss_plugin_abi`.

**Loading.** At start only, from the policy. The node reads the file once into memory and hashes it. It refuses unless the hash is in the policy's allow-list, or a detached Ed25519 signature verifies against a key the policy trusts (the policy may name keys so that an upgrade need not change every node's policy; an allow-list by hash is always enough on its own). **No code of the library runs before that check passes.** The signed message is `sha256(library) || id || version`, so a validly signed old build cannot be replayed under a new version, and a policy entry may set `min_version` so that an older signed build is refused. On Linux it writes the verified bytes to a sealed in-memory file (`memfd_create`) and loads that through `/proc/self/fd`, so there is no on-disk window at all; elsewhere it writes them to a private directory only the node's user can enter (created 0700, the file set to 0500 once written) and loads **that copy**. Either way what was verified is what runs and no replacement of the original can be loaded. The copy is not safe against the library itself: it runs as the node's user and could rewrite a file-backed copy before the next start, so the file-backed form is trust, not containment, and the node re-verifies the copy's hash at every load. It then reads `oss_plugin_abi` and the manifest (through the same parser as every tier, so `for_tier(native)` applies), checks the manifest's id and version against the policy entry, and calls `init`. These checks are consistency checks on code that is already running as the node (a constructor in the library ran at load); they are not a sandbox, and the trust is the allow-list or the key. A load failure refuses that plugin and the node starts without it. `OSS_PLUGINS` is read only when the switch of section 8 is on.

**No unload and no hot replace.** A shared library cannot be unloaded soundly: its threads, thread-local destructors and function pointers that escaped into the host may outlive a `dlclose`, and some platforms never unmap it. The node never unloads one and never replaces a loaded one. A new plugin id may be added on a signal (`SIGHUP` re-reads the policy and loads ids not yet loaded; this needs the route table behind a swappable pointer and is a later slice). A changed hash or version of a loaded id is **refused with a log line and takes effect at the next restart**; removing an id disables it (its breaker is held open and its bulkhead drained; the code stays mapped, and the host table stays valid for the life of the process, answering `denied`, because the library's own threads may still call it). `oss_plugin_shutdown` is called only at process exit. A rollout of a new version is a rolling restart, which is cheap because a node owns nothing.

**`unsafe`, and why this is not "guaranteed safe".** The loader is the one place the workspace calls foreign code. It lives in a new crate `oss-ext-native`, the only crate that holds this `unsafe` (a local allow and a `// SAFETY:` comment on each block, held by `scripts/check-unsafe.py`), behind a Cargo feature `plugins-native` so a build without it contains none of it. The code the library runs is not memory safe by Rust's rules. What stands in for the guarantee is **trust decided by the operator before load**: the allow-list or the trusted key, a private verified copy, a manifest that must match the policy, and the limits in section 6. This is an exception to ADR 0009 and to `docs/unsafe.md`, which refuse FFI outside a crate built for it without the owner's written OK, and that the owner has asked for; the `check-unsafe.py` allow-list entry for `oss-ext-native` lands in the same change as that OK, and the workspace `unsafe_code` deny stays in force everywhere else. The exception is, for this crate and this purpose only; ADR 0009 gets an "amended by 0080" line when this ADR is accepted. An operator who wants the sandbox runs the plugin as WASM.

**Memory.** A plugin's own heap is outside the host's reservations unless it uses the host's allocator. `OssHost` carries `alloc` and `free` that charge the `Extensions` pool; the SDK installs them as the plugin's global allocator, and the manifest says `"allocator": "host"`. A plugin that does not is **unmetered**: the policy refuses it unless the entry names `allow_unmetered`, and the status route shows the flag. The plugin's declared `memory_bytes` is reserved at load as its working set either way. **This metering is cooperative, not a bound**: a plugin can claim `host` and not install it, and memory it takes outside the global allocator (`mmap`, a linked C library's `malloc`, thread stacks) is not seen. The host counts the table's `alloc` calls per plugin (`host_allocs`, shown in the status route); a plugin that declares `host` and shows zero after its first calls is flagged `unmetered` as if it had declared nothing. A refused `alloc` returns null; the SDK's allocator then calls `handle_alloc_error`, which aborts, so out of memory in a host-allocated plugin is a crash counted by the guard below unless the plugin handles it.

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

The contract is defined once, in `oss-ext`, as a bytes codec with strict validation of every output; the host validates the output of every tier before use, including `native` (a plugin's output is data to the node, not code). Three adapters carry the same bytes: WASM's `oss_call(point, in_len, out_cap)`, gRPC's `Call`, and for native a C call through the table of section 1a (a trait call when linked in). Status codes are the ABI's. A point's version is matched exactly; a new point or a new optional field is additive.

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

A node's **plugin policy** (a JSON file named by `OSS_PLUGINS`, read at start, the same on every node; dynamic reload is later and the route table is built behind a swappable pointer so it can be added) lists, per plugin id: the tier and where it is (native: the library path and its SHA-256, or the id of a trusted signing key, or for a linked-in plugin its registered name; wasm: the module's SHA-256; grpc: the endpoint and, for TCP, the client certificate and the server name to verify), the **grants**, and ceilings for budget and limits. The manifest asks; the policy grants; the effective set is what is both, and an over-ask refuses the load.

Capabilities, v1:

| Capability | Gives | Tiers | Cost |
|---|---|---|---|
| `state.read`, `state.write` | tenant-scoped key-value state under `plugins/<id>/<tenant>/`, section 5 | native, wasm | the table in section 5 |
| `search` | run a search as the calling tenant through the reader | native, wasm | a search; charged to the tenant |
| `index.write` | index documents into the node's own indexes through the `_bulk` path, bounded by `budget.index_ops` | native, wasm | the writer's own cost, charged to the bulk budget |
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

**Every call has a budget.** The manifest declares the most state operations, searches and egress calls one call may make (default **zero** of each), the policy caps it, and the host counts and answers 429 `plugin_budget_exceeded` to the next one, so a plugin's cost per request is bounded by a number a reviewer can read, and that number multiplied by the request rate is what the plugin can cost, never a function of how many documents, indexes or days there are. Every operation is charged to the calling tenant's request counters, labelled with the plugin id. **Idle plugins make zero requests**: the host starts no timer, no health check and no poll for any plugin, a lifecycle event is delivered only because the event happened, and a breaker probes only when a call arrives. There is no open-ended background-job capability in v1 (ADR 0084 adds `job`, which only runs a search a call asked for); the alerting runner's slot-claim protocol is the model for a later `schedule` capability (section 9), not something a plugin can do on its own today.

A native plugin that uses the host API gets the same counting and the same numbers. The in-tree ports, which use the store layers directly under their own specs, are first-party, stay linked in, and stay under those specs. **A loaded library has no store handle at all**: no Rust type crosses the C ABI, so its only store access is the host table, which counts and budgets every operation. It can still open its own sockets and files, because it is trusted code in the node's process; that is what the allow-list and signature are for, and why untrusted code is WASM or a process.

### 6. Failure: a plugin failure is the failure of one call

The rule across all tiers: **a plugin fails a call, never the node, and a failure that would make a stored thing wrong refuses instead of falling back.**

| What went wrong | Answer | Counts toward the breaker |
|---|---|---|
| plugin refuses the input deterministically (its own 4xx status) | 400 `plugin_failed`, the plugin's detail made printable and bounded | no |
| fuel, memory, deadline or stack limit; a trap; a panic (native: caught by the SDK's export wrapper) | 400 or 500 `plugin_failed` naming the limit | yes |
| output malformed (the host's validation) | 502 `plugin_malformed` | yes |
| deadline passed (gRPC and host calls inside a wasm call included) | 504 `plugin_timeout` | yes |
| capacity: gate full, pool cannot hold the reservation, budget used | 429, nothing queued | no |
| a plugin's call that enters a plugin route or point (through `search`) | 400 `plugin_recursion`, depth is 1 | no |
| breaker open, process away, connection refused | 503 `plugin_unavailable` at once, no call made | the cause counted |

Where a plugin sits on a path that builds durable state (`analyzer`, `ingest`), its failure **refuses the operation** (503 or 4xx), never skips the plugin; where it only observes (`lifecycle`), the event is dropped and counted and the operation it observed is never delayed: delivery is from a bounded queue of 256 events a plugin, drop-newest, so a plugin that stops answering costs a counter, not a thread.

**Circuit breaker**, shared by all three tiers, pure logic in `oss-ext` with an injected clock. Failures a caller's input can cause (a trap, a limit, malformed output, a panic) count toward a **per-(plugin, tenant)** breaker (a table bounded at 1,024 entries, least recently used dropped), so one tenant's hostile input cannot open the breaker for others; failures that are the plugin's own (connection refused, transport timeout, process away) count toward the **per-plugin** breaker. Each: closed to open on 5 consecutive counted failures, or on at least half of the last 20 calls failing once 10 have been seen; open for 30 s with 20% jitter; then half-open, admitting one probe at a time; a success closes it, a failure reopens it with the time doubled up to 5 minutes. An open breaker answers 503 without calling, so a dead plugin costs nothing and cannot hold a thread. Per-plugin concurrency is bounded by a **bulkhead** (the gate of ADR 0052: 8 a plugin, at most 2 for one tenant), and a call's deadline is the least of the caller's remaining time, the point's ceiling and the plugin's limit.

**What the node can do about a native fault.** A segmentation fault, an abort or a stack overflow in loaded code ends the process, and no handler can make surviving one sound, so the node does not try. It limits what a fault costs:

- *Crash-loop guard.* Before the first call to a native plugin after load, the node writes the plugin id and its policy hash to a file in the directory named by `OSS_PLUGINS_STATE_DIR` (default under the node's data directory; node-local, not the store, so no request, and **advisory**: it is not correctness state, and losing it is safe) and removes it at a clean shutdown. A start that finds the file knows which plugins were active at an unclean exit. Two such starts in a row naming the same plugin and hash refuse that plugin (`refused: crashed twice`) until its policy entry changes, so a plugin that crashes the node on every request does not keep it in a restart loop. Two limits, stated: on an ephemeral or container filesystem the file is lost at restart and the guard does nothing there; and each node learns separately, so one plugin that crashes on one tenant's input costs up to two crashes on every node it reaches, up to 2N across a fleet.
- *Watchdog.* A thread that is started by the first in-flight call and waits on a condition variable with the earliest in-flight deadline (it exits when none is in flight, so it runs no timer and wakes for nothing when idle) compares each in-flight call with its deadline. A call past it marks the plugin `stuck`: its breaker is held open, new calls answer 503, and its place in the bulkhead is not given back. A hung thread cannot be killed soundly, so `serverless.plugins.native.stuck_action` says what follows: `quarantine` (default) leaks the thread and counts it, and after `max_stuck` of them (default 4) a plugin's action becomes `exit`; `exit` drains the node and exits with a distinct status so its supervisor restarts it.
- *Panic.* Caught by the SDK. A plugin without it that unwinds aborts the node, a crash.
- *Restart is cheap.* Either end is a cold start of one node that owns nothing, the same event as losing it, and a rolling restart is also how a new plugin version goes out.
- *What it cannot do.* A library that spawns threads, opens sockets or files, or corrupts memory is not contained. That is the cost of the tier, and why the policy allow-lists by hash.

The node holds no lock across a plugin call, so a panic or a hang poisons nothing the node needs.

**Visibility.** `GET /_plugins/_serverless/plugins` answers from the node's memory with the policy's hash (so a rollout where nodes differ is visible, and a client may see a plugin route answer 404 on one node and 200 on another until the policy is the same everywhere), and each plugin's id, tier, version, state (`ready`, `breaker_open`, `refused`), counters and last failure; `all_access`, no store request, a difference from stock OpenSearch listed in [compatibility](../compatibility.md). The same counters are in `/_prometheus/metrics`.

### 7. Memory

All three tiers draw on the `Extensions` pool of ADR 0052 (`OSS_EXTENSIONS_PERCENT`, 0 to 20, default 0): a WASM call's reservation as there; for every call, the request body and the response ceiling before either is read (a response is read into a buffer the host reserved for the point's ceiling, never grown by the plugin's word); a native plugin's own heap through the host allocator (section 1a), a refusal being an allocation failure the SDK turns into the memory-limit status; a gRPC connection's buffers once at connect. The peak a call holds is stated per tier: native, `request body + response ceiling + the plugin's declared memory_bytes`; wasm, ADR 0052's `linear memory + 2 x io_bytes + stack` plus the body and response ceiling; grpc, `request body + response ceiling + the connection's buffers (one 64 KiB window and frame pair per connection)`. A request body for a plugin route is bounded by `serverless.plugins.max_request_bytes` (default 1 MiB, at most 10 MiB) and a response by the point's ceiling (default 4 MiB); nothing streams in v1. With the pool at 0, no wasm or grpc plugin loads and the node says so at start.

### 8. Switch, defaults and the build rule

- No master switch (the owner's decision, 2026-11-28: plugin support is new code, and with no policy nothing loads, no request is made and the one route added is answered from memory, so there is no downside for a switch to hold back). `OSS_PLUGINS`, the deployer's policy, is the opt-in to running anyone's code; the native and gRPC tiers keep their own switches below.
- `serverless.plugins.grpc.enabled` (default false, requires the above): the one tier that opens outbound connections.
- Existing tests pin the old behaviour: the proxy, the ported routes, the analyzer pin and its switch are unchanged. New tests cover the new behaviour; both run in CI.
- `serverless.plugins.native.enabled` (default false, requires the first): the one tier that runs foreign code in the node's process; with it off no library is read and the `plugins-native` code path is unreachable. A plugin linked in is dormant whenever the first setting is off: it registers nothing, mounts no route and receives no event. Existing in-tree ports keep their own switches and are not governed by these until they adopt the system.

### 9. How learning to rank and the four ports could be built as dynamic plugins

None must, and none is rewritten by this work. Only alerting has a spec today; the notes on index state management, anomaly detection and ML Commons are indicative. Each can ship linked in (the same `NativeInstance`, its `routes()` registered as a `rest` point and its hooks as `lifecycle` subscriptions) and become a library later, **when what it needs from the node is expressible through the host table**, because no Rust type crosses the C ABI. What stops each from being a library today, and the addition (each a minor-version capability or point, its own spec) that would remove it:

- **Learning to rank** (not one of the four ports in flight, so slice 8 edits it): its routes use document-set stores directly under its own spec, so a library would re-express them over the host `state`, and a model such as a gradient-boosted tree ensemble can exceed 256 KiB, so it needs the same large-value key class as anomaly detection below. The `sltr` rescore is the hard part, since it runs feature queries over the reader's hits. The way across is a `rescore` point that takes one block of hits and their feature matrix (the host runs the feature queries, as it does now) and returns scores, so the model code (a tree ensemble) is the library and the reader stays the host's. Missing: that point.
- **Alerting:** routes and monitors are `rest` plus `state`; running a monitor is a `search` through the host; the scheduler is the blocker. Its slot-claim protocol (derive the next slot from the schedule and the clock, claim it by one compare-and-swap, the top two placed nodes only, at most once a slot) is the design a `schedule` capability would lift into the host so no plugin can invent a poll; it must carry the 256-marker registry and the top-2 placement, not only the claim, or a library that adopts it would break idle-zero. Missing: `schedule`.
- **Index state management:** `rest`, `lifecycle` (index created), a runner, and index administration (rollover, force merge, close). Missing: `schedule` and an `index_admin` capability.
- **Anomaly detection:** `rest`, a runner, `search`, and a model checkpoint in `state` (large values). Missing: `schedule` and a state value ceiling above 256 KiB for a declared key class.
- **ML Commons:** `rest` and `state`; local inference is CPU-heavy and benefits from being a library with its own allocator; remote-model connectors are the natural first `grpc` users, keeping credentials in the plugin's process. Missing: nothing new for the remote form.

A port moves in its own change, by its owning thread, once it is stable; what it gains is a uniform status entry, a breaker, the plugin id on its meter lines and, as a library, its own release cadence; what it keeps is its request budgets under its own spec.

This work edits none of those ports' files. New code is in `crates/ext` (contract, ABI types, breaker), `crates/ext-wasm`, new `crates/ext-native` (package `oss-ext-native`), `crates/ext-grpc` (`oss-ext-grpc`) and `crates/plugin-sdk` (`oss-plugin-sdk`), `crates/node/src/plugins/` and `plugins/` (example plugins); the only edits to shared files are an assembly hook in `crates/node` and rows in generated tables.

### 10. Crates

`oss-ext` grows the manifest, the point contract, the capability and budget types, the breaker and the C ABI types (still no runtime, no `unsafe`, no workspace dependency). A new `oss-ext-native` holds the loader, the trust check, the private copy, the watchdog and the crash-loop guard, and is the one crate with the `unsafe` of section 1a, built only with the node's Cargo feature `plugins-native`. A new `oss-plugin-sdk` is what a Rust plugin links: the exports, the panic wrapper, the host allocator and typed helpers over the table; it is published for authors and never linked into the node. `oss-ext-wasm` gains the effectful host imports, linked only when granted. A new `oss-ext-grpc` holds the client (`h2` and `prost` for the small message pair, on a runtime the crate owns: tokio and `h2` are in the tree through `oss-edge`, behind its `edge` feature, and the node itself has no async executor, so `oss-ext-grpc` creates a small fixed-size runtime lazily at the first connect, with h2 keepalive disabled and no timers, exposes only a blocking `call(deadline)`, and states its thread count), built only with a node Cargo feature `plugins-grpc`, so a build without it has no new dependency. The registry, dispatcher, policy reader, state host API and the routes live in `oss-node`'s `plugins/` module. The layering check and port map get entries for each.

## Alternatives rejected

- **Native code compiled in only** (this ADR's first text). Safe for the build but a plugin then ships with the node, which is not an Envoy-style plugin; the owner asked for run-time loading. Kept as the second form of the tier for first-party code.
- **Loading libraries with no trust check, or unloading and hot-replacing them.** The first runs whatever is in a path; the second cannot be made sound (section 1a). Rejected.
- **Running every library in a helper process.** Contains a crash, but a hop and a process to manage; the same code can already be served as a `grpc` plugin by the SDK's server shim in a later slice.
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
- A bad WASM or gRPC plugin cannot take the node down, hold its memory past a reservation, or cost requests beyond its budget; a bad native plugin can crash or hang the node, which the design says rather than pretending otherwise, and limits with an allow-list, a crash-loop guard and a watchdog.
- Harder: three transports to keep in step (golden vectors and one conformance suite run against all three), a larger build matrix (a `plugins-grpc` feature in CI), and a policy file deployers must keep identical across nodes.
- Rules out: unloading or hot-replacing a loaded library, loading one that is not allow-listed or signed, process supervision by the node, ambient capabilities, state listing, a plugin-owned timer.
- Not decided here, each its own ADR: each non-REST point's wiring, a `schedule` capability, dynamic enablement, signed WASM modules, resource limits for a library beyond its declared memory and the watchdog, a Windows loader.
- Acceptance criteria and slices are in [docs/specs/plugin-system/SPEC.md](../specs/plugin-system/SPEC.md).
