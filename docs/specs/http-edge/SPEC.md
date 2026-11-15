# HTTP edge

## Serves

[ADR 0004](../../adr/0004-bounded-memory-and-streaming.md) decision 3 (REST bodies stream) and invariants I-15 to I-17 for the REST path; [ADR 0006](../../adr/0006-http-stack.md), the HTTP stack; I-13's REST error shapes.

## Change

- `oss-rest` gains a blocking HTTP/1.1 server (`Server::start`) on a fixed worker pool, a `Router` of method plus path patterns with each route's query parameters, `Request` and `Response` types for handlers, and OpenSearch's error bodies (`RestError`, `Exception`).
- Request bodies stream to the handler one body buffer at a time (`Body::next_chunk`), from a node-wide pool charged to `Pool::RequestBodies`; a handler reserves what it keeps (`Body::reserve`). Responses stream chunked or with a length through one write buffer.
- `oss_rest::json::Tokenizer` (a push JSON tokenizer) and `oss_rest::ndjson::Splitter` (a line splitter for `_bulk` and `_msearch`) parse bodies chunk by chunk.
- Ported behaviour, each from the Java named in [port-map.tsv](../../../crates/rest/port-map.tsv): path matching, dispatch order and the 400/405/406 bodies of `RestController`; the unrecognized-parameter refusal of `BaseRestHandler`; `RestUtils` query and percent decoding; `BytesRestResponse` exception bodies; the `in_flight_requests` breaker's 429 message; Netty's head limits and 413.

Not in this change: any endpoint (the role crates register them), wiring into `oss-node`, `pretty` indentation and `human` units (accepted, not applied), `filter_path` and `error_trace` (refused as unrecognized), SMILE/YAML/CBOR bodies (406), request compression, TLS, HTTP/2, a trie router.

Differences from OpenSearch, each deliberate: a connection past `max_connections` gets 503; more than `max_headers` headers or `max_query_params` parameters gets 400; a lone UTF-16 surrogate in a JSON `\u` escape and percent-decoding to invalid UTF-8 are refused instead of replaced; `Allow` lists methods in declaration order; `100 Continue` is sent after routing and admission instead of before; duplicate JSON keys are left to the consumer.

## Acceptance criteria

- AC-1: `Server::start` charges `workers × (2 × head buffer + write buffer) + body_buffers × body_buffer_size` to the request-body pool, refuses to start with a pool one byte short, and releases it all on `shutdown`.
- AC-2: with `max_connections` open, the next connection is answered 503 with `connection: close` and closed; the ones inside are all served.
- AC-3: with every body buffer held, the next request with a body waits `body_wait`, then gets 429 `circuit_breaking_exception` with the `in_flight_requests` message, `bytes_wanted` and `bytes_limit`; once buffers return, a body is admitted again; a waiting checkout is woken by a returned buffer.
- AC-4: a body of exactly `max_content_length` is read; one byte more is refused 413 with an empty body before it is read, and the connection closed; a chunked body is refused 413 once it passes the limit.
- AC-5: the request line, header bytes, header count and query-parameter count are each accepted at their limit and refused one past it with 400 (Netty's `too_long_http_line_exception` and `too_long_http_header_exception` messages for the first two).
- AC-6: a handler receives the body in chunks of at most `body_buffer_size`, for `Content-Length` and chunked bodies and after `100 Continue`; `Body::reserve` grants up to the pool and refuses one byte more with 429, releasing everything when the request ends.
- AC-7: sixteen concurrent 4 MiB bulk uploads to eight workers with eight 64 KiB body buffers, each tokenized line by line, keep the process's heap peak within the bytes the server charged plus the handlers' reservations plus 512 KiB, and every upload is served.
- AC-8: for any JSON document, in any feed size and any string piece size from 4 to 32 bytes, the tokens rebuild the value `serde_json` reads, and no string piece exceeds its size; UTF-8 split across feeds stays whole.
- AC-9: the tokenizer's depth, key, number and string limits each accept their limit and refuse one more; comments are whitespace; malformed documents are refused with a line and column.
- AC-10: the NDJSON splitter yields the same lines for every feed size, drops a `\r` before `\n`, never holds a part larger than the feed, and refuses bytes after the last newline with `The bulk request must be terminated by a newline [\n]`.
- AC-11: a chunked response goes out in chunks of at most the write buffer; a known-length response refuses a byte too many and a short body; `HEAD` answers carry the length and no body; `X-Opaque-Id` and `X-Request-Id` are echoed.
- AC-12: an explicit segment beats a `{param}` at the same position; a path matched only by other methods is 405 with `Allow` and the `RestController` message, `OPTIONS` on it 200 with `Allow`; a path nothing matches is 400 `no handler found for uri [..] and method [..]`; an unknown method is 405 `Unexpected HTTP method`.
- AC-13: an exception rendered by the edge matches the recorded `create-index` fixture's 400 (status, content type and body) in `fixtures/compat`.
- AC-14: an unknown query parameter is refused 400 `illegal_argument_exception` with `BaseRestHandler`'s message and its Levenshtein suggestions (above 0.5, best first, ties by name); a malformed escape with `RestUtils`' message.
- AC-15: a body without `Content-Type` gets 406 `Content-Type header is missing`, one of an unsupported type 406 `Content-Type header [..] is not supported`.
- AC-16: a kept-alive idle connection is closed within a few idle polls when another connection waits for its worker, and the waiting one is served.
- AC-17: every allocation in `crates/rest/src` outside tests carries an `// ALLOC:` bound (`scripts/check-alloc.py`).
- AC-18: a body that arrives after a pause longer than `idle_poll`, after the head or after `100 Continue`, is read; a head trickled in is closed once it has taken `request_timeout`; a body read or a response write blocked longer than `request_timeout` plus `1 / min_transfer_rate` seconds per byte moved fails and the connection closes.
- AC-19: a handler that panics is answered 500 with `connection: close`; the worker serves the next connection and the open count returns to zero.
- AC-20: the content-type 406 comes before the unknown-parameter 400, as in `RestController#dispatchRequest`; two `Content-Type` headers are 400 `only one Content-Type header should be provided`; a path `java.net.URI` cannot parse is 400 `invalid uri has been requested`, not echoed.
- AC-21: an HTTP/1.0 client gets a streamed body unframed, then a close; an echoed id longer than a quarter of the write buffer is left out and the response is still sent; a server bound to an unspecified address shuts down.

## Test plan

- AC-1 to AC-5: each limit has a test at the limit and one past it; they fail if a comparison is off by one or a check is skipped.
- AC-7: the peak test is its own binary with the counting allocator; with the handler collecting the body into a `Vec` instead it read 45 MB and failed, so it catches whole-body buffering.
- AC-8: a proptest against `serde_json`; it fails on any dropped, duplicated or mis-escaped byte at a chunk or piece boundary.
- AC-12 to AC-15: expected bodies are the Java messages verbatim; AC-13 reads the recorded fixture.

## S3 request budget

Unchanged: the edge makes no S3 calls.

## Switch

None: a new crate with no default behaviour change; no node serves through it yet.

## Risks

- Thread per served connection: many idle clients cost threads. Bounded by `max_connections` and the idle yield (AC-16); ADR 0006 names the event-loop fallback.
- A peer slower than `min_transfer_rate` (64 KiB/s by default) is cut off (AC-18). OpenSearch has no such bound because a slow socket costs Netty no thread; a client that slow behind the ingress is not expected.
- A path with a percent-escape that is not UTF-8 is answered `invalid uri has been requested`; Java decodes it with replacement characters and echoes it.
- Head and chunk parsing are ours. The limits and malformed inputs are tested; `httparse` is the fallback if a parsing bug appears.
- Error messages for Netty's head limits come from Netty's source, not from a recording; a recorded fixture would confirm them.
