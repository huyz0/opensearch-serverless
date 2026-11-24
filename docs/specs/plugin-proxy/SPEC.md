# Plugin proxy: requests for plugins not ported go to a Java node

## Serves

R15.4, under [ADR 0043](../../adr/0043-plugin-requests-are-proxied-to-java-nodes.md). ISM, alerting, anomaly detection, ML Commons and learning to rank run only in the Java shell; a client talking to a Rust node still reaches them.

## Change

- `oss_rest::Router::prefix(prefix, handler)` registers a prefix route: a path equal to the prefix or under it on a whole segment, any method, tried when no ordinary route has the path and method (so also when only other methods match). `Request::headers()` lists a request's headers.
- `oss_node::plugin_proxy` holds the fixed prefix list, the upstream rotation and `ProxyHandler`, built only when `serverless.plugins.upstreams` is non-empty. Settings: `serverless.plugins.upstreams` (default empty, off), `serverless.plugins.max_request_bytes` (10 MiB), `serverless.plugins.timeout` (30 s).
- The handler sends the request to the next upstream with the method, the raw path and query, the headers but the hop-by-hop ones, and `X-Forwarded-For`; it streams the body out and the answer back through one pooled buffer.

## Acceptance criteria

- AC-1: with no upstream configured a request under a plugin prefix is today's 400 `no handler found`, and no prefix route exists.
- AC-2: with an upstream, a request under each fixed prefix (GET, POST, PUT, DELETE, a body of 1 byte, of 1 MiB, empty) reaches it with the same method, path, query, body and headers (but the hop-by-hop ones), and the client gets its status, headers (but hop-by-hop) and body unchanged.
- AC-3: an ordinary route under a plugin prefix still answers natively; a path that merely begins with the prefix's text (`/_plugins/_mlx`) is not proxied.
- AC-4: a connect failure (TLS handshake included) tries the next upstream, each at most once; every upstream refusing is 502 `plugin_upstream_unavailable`, a refusing upstream is skipped for 5 seconds, and a request whose first byte was sent is never retried.
- AC-5: a request body over `max_request_bytes`, is 413 by `Content-Length` before any connect, and a chunked one past it aborts both connections with nothing past the limit sent; the limit is the smaller of `http.max_content_length` and the setting.
- AC-6: an upstream that stalls past `timeout` before the head is 504; one that stalls mid-body (per read) closes the connection with no second status.
- AC-7: a body of 1 GiB streams in and out with peak memory within the buffer pool's reservation (peak-memory test).
- AC-8: no object-store request is made by a proxied request, and none by an idle node with the setting on.
- AC-10: a path with CR, LF, NUL, a `..` or empty segment or an encoded slash is 400 and no connection is made; the target on the wire is origin-form on the configured base whatever `Host` or `X-Forwarded-*` says.
- AC-11: the request is re-framed (one of `Content-Length` or chunked, the client's `Transfer-Encoding` dropped), headers named in `Connection` are not forwarded, and each upstream connection serves one exchange.
- AC-12: `Expect: 100-continue` with every upstream down is 502, never a 100; a ported GET beside an unported POST under a prefix proxies the POST; HEAD has no body; OPTIONS is forwarded.
- AC-9: `Authorization` reaches the upstream unchanged; a `Host` header from the client does not; a client's own `X-Forwarded-For` is appended to, never trusted alone.

## S3 request budget

0 requests, proxied or idle: a row `plugin_proxy.requests` of exact 0.
