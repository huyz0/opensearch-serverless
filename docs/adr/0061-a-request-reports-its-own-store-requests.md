# 0061: A request that asks reports the store requests it made, in a response header

**Status:** Proposed

## Context

The roadmap lists "per-request blob cost in responses" under cost guardrails. Today the node knows what each *index* costs ([ADR 0037](0037-per-index-store-usage.md): a ledger fed by the store's meter) and judges tenants by it ([ADR 0041](0041-tenant-cost-levels.md)), but a caller cannot see what one request cost. That is the number an operator needs to tune a query or a bulk shape, and the one that turns "requests scale with nodes and bytes" from a rule into something a client can check on its own call.

The meter is called on the thread that makes a store request, and the ledger already reads two thread-locals there (the owner of the key, and the indices a shared upload is split across). A request is served on one worker thread, but its store requests are not all made on it: a cold open reads files on scoped threads (`reader::prefetch::primed`), loads modules beside the commit (`search_open`), and the coordinator's fan-out drains items on scoped threads (`coordinator::fanout`). Some work for a request is shared (a page-cache fill another request started, a log PUT a commit window shares) and some is not for any request (the janitor, timers, takeover).

Forces: no extra store request ([principles](../principles.md)); bounded memory with reservation before allocation ([ADR 0004](0004-bounded-memory-and-streaming.md)); no cost on a request that does not ask; the report must never be read as a bill it is not.

## Decision

**A request that carries `X-Opensearch-Serverless-Cost: report` is answered with an `X-Opensearch-Serverless-Cost` response header counting the store requests made on its behalf on this node; a request without it pays nothing.**

1. **Scope.** `oss_blob::cost::RequestCost` is a small set of atomic counters (milli-requests by `S3Op`, bytes read, bytes written; about 100 bytes), created by the REST edge only for a request that asks, and held in a thread-local scope on the worker for the length of the request. The meter's tap adds each request and byte count to the scope too when one is set. A request that does not ask sets no scope: its cost is the thread-local read the ledger already makes beside the two it has.
2. **Carrying it across threads.** `RequestCost::capture()` returns a handle (an `Arc`) or none, and `handle.enter()` is a guard that sets the scope on another thread. Every place on a request path that hands store work to another thread captures at the spawn and enters inside it: the three sites above, and the hand-off of a queued search ([ADR 0056](0056-over-share-searches-wait-off-the-workers-in-a-bounded-queue.md)). A test per site shows its requests are counted. A site that is missed under-counts, never over-counts, and never fails the request.
3. **What is counted, and what is not.** Counted: the store requests, and their bytes, made by threads working on the request, including a page-cache fill the request itself started. Not counted: a fill another request started that this one waited for (single-flight waiters pay nothing), the janitor, timers, takeover and anything else no request caused. A write's log append is shared by the writes of its commit window; a write reports its share by the same byte weights `Ledger::split` uses for the ledger, so the shares of a window add up to its PUT. Requests another node made for a forwarded request are not counted here (the transport carries no headers); the header names this node's own count.
4. **The header.** `X-Opensearch-Serverless-Cost: get=12;get_range=3;head=0;put=0;list=0;delete=0;bytes_read=48211;bytes_written=0;units=1.040;node=<id>;scope=node`. Counts are whole numbers (a share rounds to the nearest request, in thousandths for `units`), `units` is the ledger's cost in PUT-equivalents ([ADR 0037](0037-per-index-store-usage.md)), and `scope=node` says it is a lower bound for the request's whole cost across a cluster. It is written with the response head, so it counts what was made before then; a response whose body streams after the head can make more requests, and the header then ends with `final=false`. It is never sent without the request header, and it adds no store request.
5. **Memory and bounds.** One counter set a request that asks, freed with the request; counters are fixed-size, so there is nothing to bound by records, documents or indices. A request header longer than the server's header limit is refused as today.
6. **Authorization.** The header reveals counts only, to the caller whose request produced them; it names no other tenant or index and is subject to the route's authorization as the response is.
7. **Switch.** No node setting: the request header is the switch, and a request without it is as it was. Both modes are tested. A cluster setting that refuses the header (a deployment that treats counts as sensitive) is a follow-up if one is asked for.

## Consequences

- A client can read what a search, a get or a bulk cost, and a test can assert a request's store requests from the response, which strengthens the request-budget tests: they read it from the response instead of the meter.
- The counted work depends on every thread hand-off carrying the scope; the per-site tests and a note in the crate READMEs keep that list honest, and a missed site shows as a low number, not a wrong one.
- The report is a lower bound and not a bill: shared and background work is attributed by the ledger, not here. The header says so (`scope=node`).
- Nothing changes for a request without the header, for the ledger, or for the guardrails.
- `docs/request-budgets.md`, `docs/compatibility.md` (an added header) and `crates/blob/README.md` change when it lands.

## Alternatives rejected

- **Always send the header.** It would add bytes to every response and make the lower bound look like a fact; opt-in keeps the cost where it is wanted.
- **Take the cost from the ledger by diffing counters around the request.** Other requests' work lands in the same counters; the diff is wrong under concurrency, the case that matters.
- **A request id on every store call.** Changes the store trait and every call site for a number that thread scopes give for the few hand-offs.
- **A trailer.** Not every client reads them, and the HTTP/1.1 server answers the common case with a whole body.
