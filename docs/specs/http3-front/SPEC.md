# HTTP/3 front

## Serves

[ADR 0058](../../adr/0058-modern-protocols-front.md) decisions 2, 3, 4 and 7: the HTTP/3 slice, after [HTTP/2](../http2-front/SPEC.md) and [gRPC](../grpc-front/SPEC.md). The current state is in [docs/protocols.md](../../protocols.md).

## Change

- The `oss-edge` front serves HTTP/3 on a UDP port: QUIC through `quinn`, HTTP/3 through `h3` and `h3-quinn`, ALPN `h3`, TLS 1.3 only (QUIC has no other). Each request stream is bridged to one HTTP/1.1 request on the node's own listener exactly as an HTTP/2 stream is (`translate`, `upstream` and the loopback code are shared), so the server's limits, error shapes and admission apply unchanged. The node itself translates; there is no proxy or sidecar.
- Setting: `OSS_EDGE_H3_ADDR`, a UDP socket address, empty (not opened) unless set. It uses `OSS_EDGE_TLS_CERT` and `OSS_EDGE_TLS_KEY`; it refuses to start without them. `OSS_EDGE_H3_CONNECTION_OVERHEAD` sets the per-connection charge. The other edge limits apply to it: `max_streams` and `max_connections` are shared with the TCP listeners, `handshake_timeout`, `idle_timeout` and `stream_buffer` are the QUIC handshake timeout, idle timeout and per-stream flow-control window, and `max_concurrent_streams` per connection is 16.
- QUIC 0-RTT is off: a replayed `POST _bulk` is not idempotent. Address validation is on: a client whose address is not yet validated is sent a `Retry` before the node holds any state for it.
- `Alt-Svc: h3=":<port>"; ma=86400` is added to every response the `tls` listener sends over HTTP/2 when `h3` is configured, with the port the UDP socket is bound to, and never when it is not. An HTTP/1.1 client of the `tls` listener has its bytes passed through unchanged, so it gets no `Alt-Svc`.
- Bodies stream in both directions and are never held whole (ADR 0058 decision 4); AC-6 holds it.
- The memory charged to `Pool::RequestBodies` grows by `max_connections × OSS_EDGE_H3_CONNECTION_OVERHEAD` (a QUIC connection's own state, default 64 KiB, set from the idle-connection measurement of AC-15) plus a fixed 4 MiB for the listener: quinn allocates its UDP receive batch once per endpoint, measured at about 3.1 MiB.

Not in this change: 0-RTT, HTTP/3 datagrams, WebTransport, server push, connection migration beyond what `quinn` does by default, QUIC `Retry` token lifetime tuning, certificate reload, client certificates, and any change to `oss-rest` or to a handler. gRPC over HTTP/3 is not offered: gRPC is HTTP/2 only.

## Acceptance criteria

- AC-1: with `OSS_EDGE_H3_ADDR` empty the node binds no UDP socket and charges nothing for HTTP/3; a build without the `edge` feature has no `quinn`, `h3` or `h3-quinn` in `cargo tree -p oss-node`.
- AC-2: for the request set of HTTP/2's AC-2 (a `GET` with a JSON answer, a `POST` with a body, a `HEAD`, a route answering 400, 405 with `Allow`, 406, and a refused query parameter), an HTTP/3 request gets the same status, body and `content-type` as the HTTP/1.1 request.
- AC-3: a request body of `max_content_length` is bridged; one byte more gets 413, as over HTTP/1.1 (or the stream is reset once the answer is out), and the connection serves the next stream.
- AC-4: with `max_streams` requests in flight and held by a handler, the next request stream is refused with `H3_REQUEST_REJECTED` and the ones in flight complete; afterwards a new stream is served.
- AC-5: `Edge::start` charges the HTTP/2 charge plus `max_connections × h3_connection_overhead` plus the 4 MiB listener charge when `h3` is set, refuses to start with a pool one byte short, releases it on `shutdown`, and refuses `h3` without a certificate and key, naming the setting; a loopback 503 or 429 reaches the client as that status and body.
- AC-6: four concurrent 16 MiB uploads over HTTP/3 (64 MiB in all) raise the process's heap peak by at most 4 MiB, and four concurrent 64 MiB uploads (256 MiB) raise it by no more than 256 KiB more than that, so memory does not grow with the bytes; every upload completes (counting allocator, its own test binary). The bound is stated, and the measured peak is in VERIFIED.md.
- AC-7: a client that resets a stream mid-upload, closes its connection with an application error, or goes silent until `idle_timeout` frees its loopback connection and its worker, and the open count returns to what it was.
- AC-8: a client that offers only `h2` or `http/1.1` as ALPN on the UDP port is refused; one offering `h3` is served; start fails with a message naming the setting when the certificate or key is missing, unreadable or mismatched. The `h3` and `tls` listeners serve the same certificate.
- AC-9: `:authority` reaches the handler as `Host`; connection-specific headers are dropped; a request whose `content-length` differs from its DATA bytes is not served as complete; an empty DATA frame (legal, and answered by `h3` as if it were the end of the body) does not end a body, up to 63 in a row; an uppercase name, CR, LF or NUL in a value and a `:path` that is not origin-form are each refused without reaching a handler; a response's `connection`, `keep-alive`, `transfer-encoding` and `1xx` heads never reach the client; the front sets no `X-Forwarded-For`.
- AC-10: 0-RTT is off: a client holding a session ticket from a first connection cannot send early data on the second (`into_0rtt` fails), and the second connection is served in 1-RTT.
- AC-11: a client connecting from an address the node has not validated is sent a `Retry`: the node's retry count rises by one for it, and the connection then completes and is served. A client that does not answer the `Retry` holds no connection slot.
- AC-12: the `tls` listener's HTTP/2 responses carry `alt-svc` naming the bound UDP port when `h3` is configured, and no `alt-svc` when it is not; an HTTP/1.1 client of the `tls` listener gets none.
- AC-13: a request over HTTP/3 makes the same blob requests as over HTTP/1.1, asserted by the meter over the AC-2 set against the in-process S3 emulator.
- AC-14: a response of 8 MiB streams through a window of 64 KiB without the front holding more than two stream buffers for it; `HEAD` answers carry the length and no body.
- AC-15: 400 idle HTTP/3 connections (`max_connections` 400) are held, the 401st is refused before the handshake, a connection idle past `idle_timeout` is closed, and one that does not finish its handshake in `handshake_timeout` is dropped. The heap per idle connection is measured and printed (counting allocator, client and server in one process, so the figure is both ends), and so is what the listener holds with no connection; `h3_connection_overhead` is set from the first, the 4 MiB listener charge from the second, and a test fails if half the both-ends figure is above the per-connection default or the listener holds more than its charge.
- AC-16: one stalled upload on a connection does not stall a sibling stream on the same connection.
- AC-17: a client that opens and resets streams in a burst of 1,000, and one that opens connections faster than `max_connections` allows, are each closed or refused, and the node serves the next request.
- AC-18: every allocation in `crates/edge/src` outside tests carries an `// ALLOC:` bound.
- AC-19: p50 and p99 of a small `GET` over HTTP/3, HTTP/2 over TLS and HTTP/1.1, 2,000 sequential requests each, on loopback, are printed in VERIFIED.md as provisional relative numbers. This is a record, not a threshold: QUIC in user space is expected to cost CPU on loopback.
- AC-20: `curl --http3-only` and `curl --http3` against the node answer a `GET` and a `POST`; NOT-RUN is allowed where no curl with HTTP/3 can be had, and the reason is recorded.

## Test plan

- AC-2 and AC-13 are differential against the HTTP/1.1 test client, over one table, as in HTTP/2. They fail if the bridge rewrites a status, drops a header or adds a blob request.
- AC-6 and AC-14 are the HTTP/2 memory tests over an `h3` client; with the front copying a body into a `Vec`, AC-6 reads about 256 MiB and fails.
- AC-8 and AC-10 use a throwaway certificate and a `quinn` client config built in the test.
- AC-11 reads the retry count the front exposes for tests (`#[doc(hidden)]`), the only test-only surface this adds.
- AC-15 prints the per-connection heap with `PeakWindow`; the default overhead is checked against it so a regression in `quinn` or `h3` state is a red test, not a surprise at 10,000 nodes.

## S3 request budget

Unchanged: the front makes no S3 calls, and AC-13 holds that.

## Switch

The `edge` cargo feature and the listener address, empty by default. With the address empty no socket opens and nothing is charged. No further switch: a front that is on has no effect on the HTTP/1.1 path.

## Risks

- **QUIC in user space costs CPU and memory per connection** beyond TCP and TLS. AC-15 measures memory and AC-19 records latency; the charge is set from the measurement.
- **UDP amplification and spoofed sources.** Address validation answers every new client with `Retry`, at the price of one round trip; AC-11 holds it. If that round trip is measured to matter, the setting to relax it is a decision for the owner.
- **`h3` is a 0.0.x crate.** The bridge touches its request and response types in one file; a breaking release is a small change, and the pin is exact.
- **`h3` cannot tell an empty DATA frame from the end of a body.** The front reads past 63 of them in a row, and a body with more is cut there: with a declared `content-length` the shortfall is refused, without one the client has sent a request that ends where it ends. Found by a mutant the first tests did not catch.
- **Worker capacity is not multiplied.** A stream holds a worker as in HTTP/2; AC-4 shows the bound.
