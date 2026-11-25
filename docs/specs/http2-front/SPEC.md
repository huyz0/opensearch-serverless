# HTTP/2 front

## Serves

[ADR 0058](../../adr/0058-modern-protocols-front.md) decisions 1 to 5: the first slice of HTTP/2, HTTP/3 and gRPC for the Rust node. gRPC and HTTP/3 are their own specs, after this one. The current state is in [docs/protocols.md](../../protocols.md).

## Change

- A new crate `oss-edge`, built only with the node's `edge` feature, with a small tokio runtime, running inside the node process: no external proxy or sidecar is needed. It serves HTTP/2 on two listeners and bridges every request stream to the node's own HTTP/1.1 listener over a loopback connection.
- Listeners, each empty (not opened) unless set: `OSS_EDGE_H2C_ADDR` (clear text, prior knowledge) and `OSS_EDGE_TLS_ADDR` (TLS 1.2 and 1.3, ALPN `h2` then `http/1.1`), with `OSS_EDGE_TLS_CERT` and `OSS_EDGE_TLS_KEY` (PEM files). Limits: `OSS_EDGE_MAX_STREAMS` (default `workers / 4`), `OSS_EDGE_MAX_CONNECTIONS` (256), `OSS_EDGE_HANDSHAKE_TIMEOUT_MS`, `OSS_EDGE_IDLE_TIMEOUT_MS`, `OSS_EDGE_CONNECTION_OVERHEAD` (128 KiB), `OSS_EDGE_STREAM_BUFFER` (default 64 KiB), `OSS_EDGE_THREADS` (default 2).
- Bodies stream in both directions over HTTP/2 and are never held whole; AC-6 holds it: more bytes than the heap rises by pass through. HTTP/3 and gRPC repeat this in their own specs.
- The `tls` listener also serves HTTP/1.1 to a client that does not offer `h2`, through the same bridge.

Not in this change: gRPC, HTTP/3 and QUIC, `Alt-Svc`, an `h2c` upgrade from HTTP/1.1 (the `Upgrade: h2c` request is deprecated by RFC 9113; clients use prior knowledge), server push, request compression, certificate reload, client certificates, and any change to `oss-rest` or to a handler.

## Acceptance criteria

- AC-1: with no edge listener set, the node opens no extra port and charges nothing extra to `Pool::RequestBodies`; a build without the `edge` feature has no `tokio`, `h2`, `h3`, `quinn`, `tokio-rustls` or `rustls-pemfile` in `cargo tree -p oss-node`.
- AC-2: for a fixed set of requests (a `GET` with a JSON answer, a `POST` with a body, a `HEAD`, a route that answers 400 for no handler, 405 with `Allow`, 406 for a missing content type, and a refused query parameter), an HTTP/2 request through `h2c` gets the same status, the same body and the same `content-type` as the HTTP/1.1 request, asserted over the set.
- AC-3: a request body of `max_content_length` is bridged; one byte more gets 413, as over HTTP/1.1 (or, once the answer is out, the stream is reset), and the connection serves the next stream.
- AC-4: with `max_streams` requests in flight and held by a handler, the next stream is refused with `REFUSED_STREAM` and the ones in flight all complete; after they finish, a new stream is served.
- AC-5: `Edge::start` charges `max_streams × 2 × stream_buffer + max_connections × connection_overhead` to the request-body pool, refuses to start with a pool one byte short, and releases it all on `shutdown`; a `max_streams` above `workers / 4` is a configuration error; a loopback 503 or 429 reaches the client as that status and body, as over HTTP/1.1: only the front's own `max_streams` refusal is `REFUSED_STREAM`.
- AC-6: four concurrent 16 MiB uploads over HTTP/2 (64 MiB in all) to a handler that reads a chunk at a time raise the process's heap peak by at most 2 MiB, so memory does not grow with the bytes uploaded, and every upload completes (counting allocator, its own test binary).
- AC-7: a client that resets a stream mid-upload, drops its TCP connection or sends `GOAWAY` frees its loopback connection and its worker within `request_timeout`, and the open count returns to what it was.
- AC-8: the `tls` listener negotiates `h2` for a client that offers it and `http/1.1` for one that does not, and both are answered as in AC-2; a TLS 1.1 handshake is refused; start fails with a message naming the setting when the certificate or the key is missing, unreadable or does not match.
- AC-9: `:authority` reaches the handler as `Host`; connection-specific headers (`connection`, `keep-alive`, `transfer-encoding`, `upgrade`) are dropped; `x-forwarded-for` is the peer address, and one sent by the client is replaced, not extended.
- AC-10: a client that sends 1,000 `RST_STREAM`-after-`HEADERS` pairs in a burst (rapid reset) one that sends endless `CONTINUATION` frames, one that floods `SETTINGS` and one that floods `PING` are each disconnected, and the node serves the next request.
- AC-11: a request over HTTP/2 makes the same blob requests as over HTTP/1.1, asserted by the meter over the AC-2 set against the in-process S3 emulator.
- AC-12: a response of 8 MiB streams through a window of 64 KiB without the front holding more than two stream buffers for it; `HEAD` answers carry the length and no body.
- AC-13: 400 idle h2 connections (`max_connections` 400) each cost no more heap than `connection_overhead` (counting allocator, both ends of the connection in one process), the 401st is refused, a connection idle past `idle_timeout` is closed, and a TLS connection that does not finish its handshake in `handshake_timeout` is closed.
- AC-14: one stalled upload on a connection does not stall a sibling stream on the same connection.
- AC-15: translation: a `content-length` that differs from the DATA bytes resets the stream (or is answered with an error, never served as complete); a request without one reaches the handler chunked; CR, LF or NUL in a header, an uppercase name, a `te` other than `trailers`, and a `:path` that is not origin-form (whitespace, `#`, control or high bytes, empty) are each refused without reaching a handler; two `cookie` headers arrive joined; a head over 4 KiB or 16 KiB after expansion is 431; a response's `connection`, `keep-alive`, `transfer-encoding` and `1xx` heads never reach the client; `Expect: 100-continue` works.
- AC-16: the front sets no `X-Forwarded-For`, and one the client sent reaches the handler as sent: no handler may read it as the peer's address, because the main listener is reachable by any client.
- AC-17: every allocation in `crates/edge/src` outside tests carries an `// ALLOC:` bound (`scripts/check-alloc.py`).

## Test plan

- AC-2 and AC-11 are differential: one table of requests, run through an `h2` client and through the existing HTTP/1.1 test client, compared field by field. They fail if the bridge rewrites a status, drops a header or adds a blob request.
- AC-3 to AC-5 mirror `http-edge` AC-1 to AC-4 at the limit and one past it.
- AC-6 is `http-edge` AC-7 over HTTP/2; with the front copying a whole body into a `Vec`, it reads about 128 MiB and fails.
- AC-8 uses a throwaway certificate generated in the test; the TLS 1.1 case is a raw client config. A mismatched key is a second throwaway pair.
- AC-10 writes raw frames with the `h2` crate's codec.
- Evidence a person can rerun, in VERIFIED.md: `curl --http2-prior-knowledge` against `h2c` and `curl --http2 --cacert` against `tls`.

## S3 request budget

Unchanged: the front makes no S3 calls, and AC-11 holds that.

## Switch

The `edge` cargo feature, off in the default build until its cost to build time is measured, and each listener's address, empty by default. Nothing else is switched: with no address set the front is not started.

## Risks

- **A loopback hop per stream.** A second head parse and a socket copy; they are small against a handler that reads a blob, and AC-6 holds the memory. If a profile shows them, ADR 0058 names the replacement, a stream abstraction in `oss-rest`.
- **Worker capacity is not multiplied.** A stream holds a worker as a connection did; AC-4 shows the bound.
- **HTTP/2 denial-of-service classes** (rapid reset, continuation flood, settings floods). `h2` mitigates the known ones; AC-10 holds two of them.
- **Certificate handling is new in this repo.** Keys are read at start and never logged; AC-8 holds the refusals.
