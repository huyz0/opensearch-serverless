# 0006: A small blocking HTTP/1.1 server of our own, on a bounded worker pool

**Status:** Proposed, 2026-11-15.

## Context

Every role serves the OpenSearch REST API through `oss-rest`. [ADR 0004](0004-bounded-memory-and-streaming.md) requires that request and response bodies stream in bounded chunks, that every buffer is reserved from the node budget before it is used, and that overload turns into 429s or backpressure, never into memory growth. [ADR 0005](0005-io-model.md) made object-store I/O blocking, with fan-out through one bounded executor, and left the REST server's I/O model open.

Forces, with the numbers that exist:

- **Connections per node are hundreds, not tens of thousands.** Clients reach a node through an ingress or load balancer that pools connections, and a node's useful request concurrency is bounded by its cores and its write log, not by how many sockets it can hold open. A blocking worker costs one thread: with 256 KiB stacks, 256 workers take 64 MiB of address space and little resident memory.
- **Memory must be bounded by construction.** OpenSearch's Netty stack aggregates a body up to `http.max_content_length` (100 MiB) before the handler sees it, and charges the `in_flight_requests` breaker after the fact. The Java shell measured 6 concurrent 95 MB bulks at +2.7 to +4 GB of live heap before it switched to demand-driven streaming. Here a body has to reach the handler in fixed chunks, with the bytes in flight known before the first read.
- **The handlers are blocking.** The engine and the store are synchronous (ADR 0005); a handler that parses a bulk body and appends to the write log blocks on both. An async server would hand each request to a blocking pool anyway, and would need a bridge between an async body stream and a blocking parser.
- **The wire format is small.** HTTP/1.1 with `Content-Length` and chunked bodies, keep-alive and pipelining, `Expect: 100-continue`. No TLS at the node (the ingress terminates it), no HTTP/2 (OpenSearch's default transport does not serve it either).
- **Builds stay small** ([docs/ci.md](../ci.md)), and the crate must compile on Linux, macOS and Windows.

| Option | For | Against |
|---|---|---|
| **Hand-rolled HTTP/1.1 on `std::net`, fixed worker pool** (chosen) | No dependency; every buffer is one we size and charge at startup; the body is read straight into a pooled buffer, so streaming and limits are exact; head parsing in place allocates nothing; error precedence ported from Netty and `RestController` | We own about 2,000 lines of HTTP framing, routing and error code, and their tests; one thread per served connection |
| `tiny_http` | Small, blocking, mature API | Spawns threads on demand (no hard cap without patching), buffers request heads in its own `String`s and `Vec`s, gives no control over head limits or the 100-continue decision, and hides the socket, so idle keep-alive connections cannot yield; its maintenance has slowed |
| `httparse` for the head, our own server around it | A fuzzed, zero-allocation head parser | Saves about 150 of those lines; adds a dependency for the one piece our tests already cover at the limits; kept as the fallback if the parser shows bugs |
| `hyper` on tokio, handlers on `spawn_blocking` | Scales to many thousands of idle connections; HTTP/2 later | An async runtime and a large dependency tree in every build, for a concurrency the node does not have; bodies arrive as hyper's own buffers, so pooled-buffer reads and an exact reserve-before-read need a bridge; a blocking pool still sits behind it |
| `mio` event loop with our own parser | Many idle connections on few threads | All the parsing work of the chosen option, plus a readiness state machine per connection and handlers moved to a pool; justified only by a connection count we do not see |

## Decision

1. **The server is ours, on `std::net`, blocking.** One accept thread and `workers` worker threads, fixed at startup. A worker serves one connection at a time, for as long as it stays open. No async runtime, at the edge or behind it.
2. **Every resource is bounded and charged at startup.** Each worker holds a head buffer, a read buffer and a write buffer, and the node holds `body_buffers` body buffers; all of them come from `oss-mem` buffer pools charged to `Pool::RequestBodies`, so the server does not start if the pool cannot hold them. Open connections are capped at `max_connections`; the ones beyond `workers` wait in a bounded queue, and one more is answered 503 and closed. Heads are capped by OpenSearch's limits (4 KiB line, 16 KiB headers), plus a header count and a query-parameter count.
3. **A body is admitted before it is read.** A request with a body takes a body buffer first, waiting up to `body_wait` (30 s by default) for one, and is refused with 429 `circuit_breaking_exception` after that; `100 Continue` is sent only once the buffer is held, so a waiting client uploads nothing. A body over `max_content_length` is refused with 413 before a byte is read (chunked: as soon as it passes). The handler reads the body one buffer at a time, and reserves from the same pool anything it keeps.
4. **Slow peers are cut off, gently, and only for their own slowness.** A head must arrive within `request_timeout` (30 s). Once a body or response is under way, only time spent blocked in a socket read or write counts: time this side spends processing a bulk, waiting for a body buffer or holding back under write-log backpressure never does, because the peer's bytes queue in the socket meanwhile and the next read returns at once. Each byte moved earns time at `min_transfer_rate` (8 KiB/s), and a peer is disconnected only when it falls `slow_grace` (10 s) behind that floor: a stalled peer after 10 s, one at half the floor after 20 s. The recommended deployment puts a buffering load balancer or proxy (ALB, NGINX with request and response buffering) in front of the nodes, so slow internet clients never reach a worker and this limit is only a safety net. A handler that panics is answered 500 and its worker carries on.
5. **Idle connections yield.** A worker whose kept-alive connection is idle while connections wait closes it within one `idle_poll` (50 ms) and takes the next, so the thread-per-connection model cannot be starved by idle clients. Clients retry a request on a closed idle connection, as HTTP/1.1 allows.
6. **Bodies are parsed by streaming parsers** in this crate: a push JSON tokenizer and an NDJSON line splitter that hold one fixed scratch buffer each and hand values on in bounded pieces.

## Consequences

- Memory at the edge is known from the configuration: with the defaults (128 workers, 128 body buffers of 64 KiB), about 15 MiB, charged when the server starts. A peak-memory test holds it under load.
- Handlers are plain blocking functions over a `Request` and a `Response`; they call the store and the engine directly.
- Every HTTP behaviour is code here, with tests at each limit: framing, keep-alive, pipelining, 100-continue, the error shapes.
- Deploy behind a buffering load balancer or proxy: it absorbs slow clients, so a worker only ever talks to a fast local peer, and the slow-peer cutoff (decision 4) stays a safety net rather than something clients meet.
- What it rules out for now: HTTP/2, TLS at the node, request compression (`Content-Encoding`), and tens of thousands of open connections per node. If a deployment needs more than a few thousand connections per node, or the accept queue's wait shows in p99, this is revisited by moving connection readiness to an event loop (`mio`) in front of the same workers; handlers, which see only `Request` and `Response`, do not change.
