# oss-rest

The HTTP edge every role serves through: the server, streaming request-body parsing (JSON and NDJSON) in bounded chunks, streaming response writing, and route dispatch. It knows HTTP and bodies, not search semantics.

## Owns
The HTTP edge: server, streaming request and response bodies, routing.

## Depends on
`oss-mem` for request-body reservations and buffers.

## Entry points
`src/lib.rs`. `Server::start` serves a `Router` (`src/router.rs`) of `Handler`s, each given a `Request` with a streaming `Body` (`src/body.rs`) and a `Response` (`src/response.rs`). `json::Tokenizer` and `ndjson::Splitter` parse bodies chunk by chunk. `RestError` builds OpenSearch's error bodies. Design: [ADR 0006](../../docs/adr/0006-http-stack.md); spec: [docs/specs/http-edge](../../docs/specs/http-edge/SPEC.md).
