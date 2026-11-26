# oss-edge

The protocol front: HTTP/2 over clear text (`h2c`) and HTTP/2 with HTTP/1.1 over TLS (ALPN `h2`, `http/1.1`), served from one small tokio runtime and bridged stream by stream to the node's HTTP/1.1 server ([ADR 0058](../../docs/adr/0058-modern-protocols-front.md), [spec](../../docs/specs/http2-front/SPEC.md)). HTTP/3 over QUIC (`quinn`, `h3`, ALPN `h3`, 0-RTT off, `Retry` address validation) is served on its own UDP port the same way ([spec](../../docs/specs/http3-front/SPEC.md)). Built only with the node's `edge` feature.

## Owns
Listeners, TLS configuration, HTTP/2 connection and stream limits, the request and response translation, and the memory charge for all of it.

## Depends on
`oss-mem` (the budget the front charges at start). `oss-rest` and `oss-testkit` only in tests.

## Entry points
`src/lib.rs`:

- `EdgeConfig::new(upstream, workers)` with listener addresses and bounds; `validate()` and `charge()`.
- `Edge::start(config, &MemoryBudget)` binds the listeners and returns an `Edge` with `h2c_addr()`, `tls_addr()`, `h3_addr()` and `shutdown()`.
- `EdgeError`: config, memory, TLS and I/O failures, each a refusal to start.
