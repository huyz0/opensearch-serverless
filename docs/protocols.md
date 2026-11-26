# Client protocols: what each side serves

What a client can speak to the Rust node and to the Java shell (`huyz0/OpenSearch`, `feature/serverlessshell`), with the evidence for each cell. The plan that closes the gaps is [ADR 0058](adr/0058-modern-protocols-front.md) and the [HTTP/2 front spec](specs/http2-front/SPEC.md).

## Matrix

| | Rust node | Java shell, stock config | Java shell, what it can do with settings |
|---|---|---|---|
| HTTP/1.1 | yes | yes | yes |
| HTTP/2 clear text (h2c) | **yes**, in a build with the `edge` feature and `OSS_EDGE_H2C_ADDR` set | yes, on both of its HTTP transports | already on |
| HTTP/2 over TLS (ALPN `h2`) | **yes**, with the `edge` feature, `OSS_EDGE_TLS_ADDR` and a certificate and key | **no**, the shell has no TLS of its own | yes, once a plugin supplies TLS parameters (the security plugin does) |
| HTTP/3 (QUIC, ALPN `h3`) | **no** | **no**, off by default and TLS only | yes: `http.protocol.http3.enabled: true` with TLS parameters, and a platform with the QUIC native library |
| gRPC | **partly**: `OSS_EDGE_GRPC_ADDR` serves the health service and refuses the rest as `UNIMPLEMENTED`; Bulk and Search follow ([spec](specs/grpc-front/SPEC.md)) | **yes** with `aux.transport.types: [transport-grpc]` and the plugin; before the shell change below it registered and never started | one setting |

## Evidence

### Rust node

Run on 2026-11-25 against `oss-node` built with `--features reader,writer`, on the filesystem store, HTTP at `127.0.0.1:19200`; `curl 8.5.0` with nghttp2, built without HTTP/3.

| Probe | Result |
|---|---|
| `curl http://…/` | `1.1 400` (no handler for `/`): HTTP/1.1 answers |
| `curl --http2-prior-knowledge http://…/` | connection closed, curl exit 56: the HTTP/2 preface is read as a malformed HTTP/1.1 head |
| `curl --http2 http://…/` (h2c upgrade) | `1.1 400`: the `Upgrade: h2c` header is ignored and the answer is HTTP/1.1 |
| `curl -k https://…/` | times out, curl exit 28: the TLS hello is waited on as a head |
| gRPC call (`content-type: application/grpc` over prior-knowledge h2) | connection closed, curl exit 56 |

Code: `crates/rest/src/server.rs` and `conn.rs` are a blocking HTTP/1.1 server on `std::net::TcpStream`; the dependency tree has `rustls` only through the outbound `ureq` clients; [ADR 0006](adr/0006-http-stack.md) rules HTTP/2 and inbound TLS out "for now". No `tokio`, `h2`, `quinn`, `tonic` or `prost` is in `Cargo.lock`.

### Java shell

By code, at `feature/serverlessshell` `e71d4233`. The shell picks its HTTP transport in `NodeServices.withShellDefaults`: `netty4` when `serverless.http.streaming` is false, and `reactor-netty4` when it is true, which is the default (`BulkSettings.DEFAULT_STREAMING`).

- **`netty4`** (`modules/transport-netty4`, `Netty4HttpServerTransport`): HTTP/1.1 with both h2c upgrade (`Http2ServerUpgradeCodec`) and prior knowledge (`CleartextHttp2ServerUpgradeHandler`). Upstream tests it in `Netty4Http2IT`.
- **`reactor-netty4`** (`plugins/transport-reactor-netty4`): `HttpProtocol.HTTP11` and `H2C` in clear text; `HTTP11` and `H2` over TLS with ALPN `h2`, `http/1.1` (`ReactorNetty4HttpServerTransport#configureHttp11orHttp2`).
- **HTTP/3**: `Netty4Http3ServerTransport` and, for `reactor-netty4`, `configureHttp3`; both need a `SecureHttpTransportSettingsProvider` (TLS parameters, which only a plugin supplies) and `http.protocol.http3.enabled`, default false. Upstream tests it in `Netty4Http3IT`. The platform needs Netty's native QUIC library, which the module's runtime dependencies carry for linux and macOS on x86-64 and arm64 and for windows x86-64.
- **gRPC**: `modules/transport-grpc` is an auxiliary transport (`aux.transport.types: [transport-grpc]`, port range `aux.transport.transport-grpc.port`), serving `DocumentService.Bulk` and `SearchService.Search`, `CreatePit` and `DeletePit` from `opensearch-protobufs`, over Netty HTTP/2 with optional TLS. Upstream's `Node` starts auxiliary transports through `NetworkModule#getAuxServerTransportList`; the shell builds its own `NetworkModule` (`NodeServiceBuilders#buildNetworkModule`) and nothing under `serverless/` reads that list, so the transport is registered when its plugin loads and never started.
- **TLS in the shell**: none of its own; the security plugin supplies it (`serverless/README.md`, `tlsTest`).

**Run live, 2026-11-25** (`ServerlessProtocolsTests`, `ServerlessGrpcTests` in `serverless/testkit`, on the shell change that starts auxiliary transports): an HTTP/2 request over clear text is answered as HTTP/2, and the same as HTTP/1.1, on `netty4` and on `reactor-netty4`; a gRPC health call to a shell node loading the stock `transport-grpc` plugin returns `SERVING`; the auxiliary-transport lifecycle test failed before the change (started 0 times, expected 1) and passes after. **Not run:** HTTP/2 over TLS and HTTP/3 on the shell; they rely on upstream's own tests (`Netty4Http2IT`, `Netty4Http3IT`) and on TLS parameters from a plugin.

## Gaps, in order

1. **Rust: HTTP/2 and inbound TLS.** Done (the `edge` feature). [ADR 0058](adr/0058-modern-protocols-front.md), decisions 1 to 5.
2. **Java: start the gRPC auxiliary transport.** Done (the shell change above).
3. **Rust: gRPC**, on the HTTP/2 front, serving the services upstream serves. ADR 0058, decision 6.
4. **Rust: HTTP/3.** QUIC and a certificate; last because it needs everything above.
5. **Java: TLS of its own**, if a deployment wants HTTP/3 or ALPN `h2` without the security plugin. Not planned: it needs a decision on where certificates come from; options are in the spec.
