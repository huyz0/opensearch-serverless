# oss-rest

The HTTP edge every role serves through: the server, streaming request-body parsing (JSON and NDJSON) in bounded chunks, streaming response writing, and route dispatch. It knows HTTP and bodies, not search semantics.

## Owns
The HTTP edge: server, streaming request and response bodies, routing.

## Depends on
`oss-mem` for request-body reservations and buffers.

## Entry points
`src/lib.rs`. `Server::start` serves a `Router` (`src/router.rs`) of `Handler`s, each given a `Request` with a streaming `Body` (`src/body.rs`) and a `Response` (`src/response.rs`). `json::Tokenizer` and `ndjson::Splitter` parse bodies chunk by chunk. `RestError` builds OpenSearch's error bodies. Design: [ADR 0006](../../docs/adr/0006-http-stack.md); spec: [docs/specs/http-edge](../../docs/specs/http-edge/SPEC.md).

## Deployment

Run nodes behind a buffering load balancer or proxy (ALB, or NGINX with request and response buffering on). It absorbs slow internet clients, so each worker thread only talks to a fast local peer. The node's own slow-peer cutoff (`min_transfer_rate` 8 KiB/s, `slow_grace` 10 s) is a safety net; it charges only time spent blocked on the peer, never time the node spends processing or holding back a big bulk. See [ADR 0006](../../docs/adr/0006-http-stack.md).
