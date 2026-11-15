# oss-rest

The HTTP edge every role serves through: the server, streaming request-body parsing (JSON and NDJSON) in bounded chunks, streaming response writing, and route dispatch. It knows HTTP and bodies, not search semantics.

## Owns
The HTTP edge: server, streaming request and response bodies, routing.

## Depends on
`oss-mem` for request-body reservations and buffers.

## Entry points
`src/lib.rs`. The server, body parser and router land in stage 2.
