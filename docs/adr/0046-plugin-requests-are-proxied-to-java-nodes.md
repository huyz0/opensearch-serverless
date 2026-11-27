# 0046: Requests for plugins that are not ported are proxied to Java nodes, behind a setting

**Status:** Accepted after independent review, 2026-11-24 (the review's ten changes are folded in below). Learning to rank is superseded by [ADR 0071](0071-learning-to-rank-is-ported-natively.md): it is ported natively, and its two prefixes leave the list when its routes land

## Context

Rust cannot load Java plugins. k-NN and security get native ports; ISM, alerting, anomaly detection, ML Commons and learning to rank keep working through the Java shell, which runs the stock plugins (parity plan R15.4). A client that talks to a Rust node must still reach them, so the Rust REST edge has to hand those requests to a Java node and hand the answer back. Today a Rust node answers every such path with 400 `no handler found`.

The lease a node publishes names a transport address and roles, not an HTTP address, so a Rust node cannot discover where a Java node serves REST.

## Decision

**Routing.**
- The proxy is a REST fallback, not a route per plugin endpoint. `oss_rest::Router::prefix(prefix, handler)` registers a prefix route: a path equal to the prefix or under it on a whole segment (`/_plugins/_ml/x`, never `/_plugins/_mlx`), any method. It is tried when no ordinary route matches the path, and also when the path matches ordinary routes of other methods only, so a ported GET beside an unported POST under a plugin prefix is proxied rather than answered 405. A method an ordinary route has stays that route's: ported endpoints win.
- The prefixes are fixed in code for the plugins not ported: `/_plugins/_ism`, `/_opendistro/_ism`, `/_plugins/_alerting`, `/_opendistro/_alerting`, `/_plugins/_anomaly_detection`, `/_opendistro/_anomaly_detection`, `/_plugins/_ml`, `/_plugins/_ltr`, `/_ltr` (the last two leave the list under [ADR 0071](0071-learning-to-rank-is-ported-natively.md)). A prefix leaves the list when its plugin is ported.
- `OPTIONS` and `HEAD` are forwarded as sent (a `HEAD` answer has no body).

**Upstream.**
- The upstream is configuration, not discovery: `serverless.plugins.upstreams`, a list of `http://` or `https://` bases without userinfo (checked when the setting is read; a load balancer's address is one). `https://` is supported and recommended, since credentials cross the link. Empty, the default, registers no prefix, so every request keeps today's 400: off, and existing tests pin that.
- The target is only ever the configured base plus the client's raw path and query, in origin form. Never absolute form, and never derived from `Host` or any `X-Forwarded-*`. A path or query holding CR, LF, NUL, a `..` or empty segment, or an encoded slash, backslash or dot-dot is refused 400 before any connection.
- Upstreams are tried in turn per request; one that refused a connect is skipped for the next 5 seconds. A connect failure (nothing sent, TLS handshake included) tries the next upstream, each at most once; all failing is 502 `plugin_upstream_unavailable`. Nothing is retried once the request head was written, so a write is never applied twice.

**Framing and headers.** The server's own parsing is relied on for the client side (`oss_rest::checks` already refuses a conflicting or duplicate `Content-Length` and a `Content-Length` beside `Transfer-Encoding`).
- The request is re-framed, never copied: the client's `Content-Length` and `Transfer-Encoding` are dropped, and the proxy sends `Content-Length` when it was known and chunked when not, re-encoding chunks it decoded. The upstream connection is used for one exchange and not kept (the client keeps no idle connections), so no error or early response leaves bytes for the next request.
- Headers named in `Connection`, and `Connection`, `Keep-Alive`, `Proxy-*`, `TE`, `Trailer`, `Transfer-Encoding`, `Upgrade` and `Host` are not forwarded either way. `Host` is the upstream's; the client's goes as `X-Forwarded-Host`. `X-Forwarded-For` has the peer address appended to a client-supplied value, and `X-Forwarded-Proto` is set. All other request headers pass, `Authorization` unchanged, so the Java node authenticates and authorises. Response headers pass back through `Response::start`'s header slice; a response header holding CR or LF is dropped. `Request` gains a header iterator for this.
- `Expect: 100-continue` is answered by the server before the handler runs, as for every route; a failed connect after it is a 502 as final status, which HTTP allows. 1xx answers from the upstream are skipped by the client; an upstream `Connection: close` ends the exchange as above.

**Bounds.**
- The request body streams from the server's pooled body buffer; the response streams back through a 16 KiB buffer reserved from the request-body pool; neither is held whole. The limit is the smaller of `http.max_content_length`, which the server enforces before reading and mid-stream for chunked, and `serverless.plugins.max_request_bytes` (default 10 MiB). Over it by `Content-Length` is a clean 413 before any connect; a chunked body over it is 413 with the connection closed, the request to the upstream abandoned.
- `serverless.plugins.timeout` (default 30 s) is the wait for the upstream's connect and for its response head: past it, 504. After the response head went out the timeout is per read, and a stall closes the connection rather than sending a status.

**Cost.** The proxy makes no object-store request, so the S3 bill does not change; an idle node makes none.

## Consequences

- A plugin keeps working through a Rust node with one extra hop, as fast as the Java node answers.
- The upstream list is operator-set; a Java node that changes address needs the setting changed (or a stable name in front of it). Discovery from leases waits until the shell publishes its HTTP address.
- The proxy adds no authentication of its own and trusts the upstream with what the client sent, so the upstream must be a node of the same deployment, reached over a private network or TLS.
- A request goes to whichever upstream is next; a plugin that needs sticky sessions is not supported until one is asked for.
