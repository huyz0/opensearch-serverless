# Search queue

## Serves

[ADR 0056](../../adr/0056-over-share-searches-wait-off-the-workers-in-a-bounded-queue.md): a tenant's searches over its share of the REST workers wait off the workers in a bounded queue, and are refused with 429 past its size. [ADR 0047](../../adr/0047-search-slots.md)'s gate is the alternative. Roadmap R13.

## Change

- `oss-rest`: `SearchAdmission` (a tenant-blind hook a router carries: `decide(method, path)` returns `Run(permit)` or `Defer(tenant)`, `try_reserve(tenant)` returns a permit when the tenant may run now) and `Router::with_search_admission`. `conn.rs` calls the hook after the guard, authorizer, content-type and parameter checks and before a body buffer. On `Defer` the server parks the connection (stream plus the head and read-ahead bytes, at most one head buffer) and the worker returns to its channel; a parked connection resumes by replay, as a new connection whose input is those bytes, holding the permit the offer reserved. A permit drops when the request's response is done, and the worker then offers the queue's next eligible entry.
- `ServerConfig::search_queue_size` (0: off, no hook is called) and `search_queue_wait`. A start refuses a size above `max_connections - workers` and reserves `size * head buffer` from `request_bodies` for the server's life.
- Per-tenant FIFO queues with a ring, offered with the same predicate admission uses. Expiry (429 at the wait), a closed peer (dropped) and shutdown (503) are handled by idle workers and by search ends, no thread added. A full queue answers 429 `es_rejected_execution_exception` with `Retry-After` and closes.
- `oss-node`: `OSS_SEARCH_QUEUE_SIZE` (0), `OSS_SEARCH_TENANT_SHARE_PCT` (25), `OSS_SEARCH_RESERVED_WORKERS` (8), `OSS_SEARCH_QUEUE_WAIT_MS` (30,000), `OSS_HTTP_MAX_CONNECTIONS` (1,024). The node's admission keys a tenant by the first plain index name of `/{index}/_search`, as ADR 0047, and runs a search at once when its tenant runs fewer than its share or the node runs fewer than `workers - reserved` searches. The usage route reports `search_queue`. A start with this and `OSS_SEARCH_SLOTS` both on is refused.

Not here: searches by alias, wildcard, bare `/_search` or `_msearch` (not tenant-keyed, run unqueued), and non-search routes.

## Acceptance criteria

- AC-1: with the queue off nothing changes: no hook is called and a request is served as before.
- AC-2: a search that is over its share while workers are scarce is parked and its worker serves another connection at once; it is answered when a worker frees, with the right body (a request with a body and a pipelined request survive the replay).
- AC-3: with the queue full the next search is answered 429 `es_rejected_execution_exception` with `Retry-After`, and the connection closes.
- AC-4: a parked search not resumed within the wait is answered 429 and its counts return.
- AC-5: a peer that closes while parked is dropped and its counts return; no permit is left held on the exits tested; a replay a guard denies drops its permit and the worker offers again.
- AC-6: shutdown answers every parked entry 503 and closes it.
- AC-7: an offer reaches workers that are all idle in the channel within one idle poll, and the offer scan and admission agree: a tenant over its share is offered when the node is under `workers - reserved`.
- AC-8: the node's settings default to off, 25, 8, 30,000 and 1,024 and refuse text, out-of-range values, a queue above `max_connections - workers`, and both this queue and the gate on; the usage route reports `search_queue` only when on.
- AC-9: a booted node with the queue on serves searches as before and the quiet tenant's search is served while a noisy tenant's searches hold their share (the cold-cache test, hand-run, in VERIFIED.md).
- AC-10: `docs/compatibility.md` records the 429 from a node rule, `Retry-After`, and the closed connection.

## Test plan

AC-1 to AC-7: `crates/rest/tests/search_queue.rs` over a real server and a test admission, each seen failing before its code. AC-8, AC-9: `crates/node/src/boot/config/tests/resident.rs` and `crates/node/tests/server/boot_search_queue.rs`; the measurement in `crates/node/tests/search_slots_cold.rs`.

## S3 request budget

None added: the queue is in memory, per node, no store request, no timer. LIST 0, HEAD 0, PUT 0.

## Switch

`OSS_SEARCH_QUEUE_SIZE`, default 0. The default stays off until the cold-cache test shows the quiet tenant's p99 improved and search p99 below saturation within 5%.
