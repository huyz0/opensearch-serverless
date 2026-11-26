# gRPC front

## Serves

[ADR 0058](../../adr/0058-modern-protocols-front.md) decision 6 (gRPC) and decision 4 (streaming in every protocol), on the HTTP/2 front of [http2-front](../http2-front/SPEC.md). HTTP/3 is its own spec. State today: [docs/protocols.md](../../protocols.md).

## Change

- A `grpc` listener in `oss-edge`, `OSS_EDGE_GRPC_ADDR`, empty (not opened) unless set. It speaks HTTP/2 with ALPN `h2` over TLS when `OSS_EDGE_TLS_CERT` and `OSS_EDGE_TLS_KEY` are set, and clear text otherwise. It serves only `content-type: application/grpc` and `application/grpc+proto`; any other service or method is `UNIMPLEMENTED`.
- Services, the ones upstream OpenSearch's gRPC transport serves (`opensearch-protobufs`, pinned commit recorded in `crates/edge/proto/PIN`): `DocumentService/Bulk`, `SearchService/Search`, `SearchService/CreatePit`, `SearchService/DeletePit`, `SearchService/ServerStreamSearch`, and `grpc.health.v1.Health/Check`. Reflection and `Health/Watch` are not served.
- The `.proto` files are vendored unmodified with their licence and compiled at build time with `protox` into a descriptor set (no `protoc` in the build) that `prost-reflect` loads, so messages are decoded to and encoded from dynamic messages. They are used for decoding and encoding only; the mapping to and from the node's REST JSON is ours, in `crates/edge/src/grpc/`, one function per message, and is **a stated subset** (below).
- A call is: read one length-prefixed message, decode it, map it to the equivalent REST request, send it over the loopback bridge of the HTTP/2 front (ADR 0058 decision 3), map the JSON answer to the response message, write it, then send `grpc-status` and `grpc-message` (percent-encoded) as trailers. No handler sees gRPC.
- **Covered request subset.** Bulk: `index`, `routing`, `refresh`, `pipeline`, `timeout`, `require_alias`, `x_source*`, and body items `index`, `create`, `delete` and `update` with their ids, routing and `object` bytes. Search: `index`, `size`, `from`, `x_source*`, `track_total_hits`, `sort` on fields, and the query types `match_all`, `term`, `terms`, `match`, `match_phrase`, `range`, `exists`, `ids`, `prefix` and `bool` (must, filter, should, must_not, minimum_should_match) nested to the REST depth limit. CreatePit: `index`, `keep_alive`. DeletePit: `pit_id`. Everything else in these messages, including a set field the table does not name, is refused with `UNIMPLEMENTED` and a message naming the field path (`SearchRequestBody.aggregations`), **never ignored and never approximated**. The covered set grows by adding rows with a test each.
- **Response mapping** covers every key the covered requests can make the node answer. A key the table does not know fails the call with `INTERNAL` naming it, so a gap shows as a failure and not as a silent drop.
- **Streaming.** Request and response bytes are never held whole beyond one message. A unary message is bounded by `OSS_EDGE_GRPC_MAX_MESSAGE` (default 4 MiB, at most `max_content_length`), charged to `Pool::RequestBodies` per call before the message is read, so a client that needs more splits its bulk. `ServerStreamSearch` answers with one `SearchResponse` message per page of hits, each written as it is produced, with backpressure through h2 flow control; a slow reader slows the loopback and holds one page, not the result set. Client-streaming and bidirectional calls do not exist in these services.
- Message compression is refused: `grpc-encoding` other than `identity` is `UNIMPLEMENTED`. `grpc-timeout` is honoured as the deadline of the loopback call and answers `DEADLINE_EXCEEDED`.
- **REST status to gRPC status** (the table upstream uses): 200 `OK`; 400 `INVALID_ARGUMENT`; 401 `UNAUTHENTICATED`; 403 `PERMISSION_DENIED`; 404 `NOT_FOUND`; 409 `ABORTED`; 413 `RESOURCE_EXHAUSTED`; 429 `RESOURCE_EXHAUSTED`; 499 `CANCELLED`; 500 `INTERNAL`; 501 `UNIMPLEMENTED`; 503 `UNAVAILABLE`; 504 `DEADLINE_EXCEEDED`; any other 4xx `FAILED_PRECONDITION`, other 5xx `INTERNAL`. The REST error JSON is carried as the `grpc-message`, truncated to 4 KiB.
- `Health/Check` answers `SERVING` when the node's `/_cluster/health` route answers 200 over the loopback and `NOT_SERVING` otherwise; an unknown `service` name is `NOT_FOUND`.
- Unchanged: HTTP/1.1, the HTTP/2 front's listeners and limits (`max_streams`, `max_connections`, windows and the pool charge, which now also counts `max_streams x 4 x max_message` for the grpc listener: the message, its decoded form, the REST body and the answer).

## Acceptance criteria

- AC-1: with no `OSS_EDGE_GRPC_ADDR` set, no port opens and nothing extra is charged to `Pool::RequestBodies`; a build without `edge` refuses the setting at start.
- AC-2: a `grpc.health.v1.Health/Check` call over clear text and over TLS returns `SERVING` on a running node and `NOT_SERVING` while the loopback route answers non-200.
- AC-3: a `DocumentService/Bulk` call with index, create, update and delete items over a real node answers the same per-item results as the same four actions over REST `_bulk`, and the documents are afterwards readable by `SearchService/Search`.
- AC-4: a `SearchService/Search` call for each covered query type answers the same hit ids, in the same order, as the REST `_search` with the equivalent body, on the same data; the test table has one row per query type and fails when a row is removed without its type being removed from the covered list.
- AC-5: `CreatePit` returns a PIT id that `Search` can use and `DeletePit` releases; the REST answers are the same.
- AC-6: a set field outside the covered subset is `UNIMPLEMENTED` with its path in the message, for each of: `aggregations`, a `knn` query, `script_fields`, a bulk `extra_field_values`; and no request reaches the loopback (asserted by a counting upstream).
- AC-7: a REST answer containing a key the response table does not know fails with `INTERNAL` naming the key (tested against a stub upstream), and a property test over the REST answers of the test corpus finds no such key.
- AC-8: the status table: a loopback answer of each status in the table maps to its gRPC status with the REST error body in the message, and a status outside the table maps as stated; the 429 `circuit_breaking_exception` keeps its body.
- AC-9: a message larger than `max_message` is `RESOURCE_EXHAUSTED` before it is buffered (the heap does not rise by the message size), and a compressed message is `UNIMPLEMENTED`.
- AC-10: `grpc-timeout: 50m` on a call to a handler that holds is `DEADLINE_EXCEEDED` within 2 s and releases its stream slot.
- AC-11: `ServerStreamSearch` over an index whose hits total more than `max_message` times four delivers every hit across several messages, in order, and the heap peak during the call rises by at most 2 MiB (counting allocator, own test binary); a client that reads slowly does not raise it.
- AC-12: calls past `max_streams` are `UNAVAILABLE` with `grpc-status` in the trailers, as `REFUSED_STREAM` is for HTTP/2; the HTTP/1.1 path serves meanwhile.
- AC-13: an unknown service or method is `UNIMPLEMENTED`, and a request with another content-type is refused with HTTP 415.
- AC-14: the official Java client stubs of `opensearch-protobufs` (the shell's `transport-grpc` test client) and `grpcurl` with the vendored protos each complete a Bulk, Search and Health call against the Rust node (NOT-RUN is allowed here if neither can be fetched; the reason is recorded).
- AC-15: a call makes the same blob requests as the equivalent REST call, asserted by the meter over the AC-3 and AC-4 sets against the in-process S3 emulator.

## Test plan

- Wire level (AC-2, AC-9, AC-10, AC-12, AC-13): a `h2` client in `crates/edge/tests/grpc.rs` writes frames by hand, against a real node. Each starts red because the listener does not exist.
- Mapping (AC-3 to AC-8): differential tests, gRPC against REST on one node and one data set; the covered-types table is read by a test that also reads the mapping's own list, so they cannot drift.
- Streaming and memory (AC-9, AC-11): own test binary with the counting allocator, as the peak-memory test of the HTTP/2 front.

## S3 request budget

Unchanged: the front makes none and a call reaches the same handler as its REST twin (AC-15).

## Switch

`OSS_EDGE_GRPC_ADDR`, empty by default; with it unset the listener is not started and costs nothing (decision 7 of the ADR). The `edge` feature gate is unchanged.

## Risks

- **Mapping fidelity.** The protobuf schema is large (about 200 messages in one file) and its JSON form is not protojson; a hand mapping drifts from upstream's. The covered subset is explicit, a field outside it is refused by path, and AC-4 and AC-14 compare against REST and against upstream's own client. The first slice does not claim more than the table.
- **Whole-message decode.** A protobuf message is decoded whole, so a call costs up to `max_message` of the pool; AC-9 and the pool charge bound it. Streaming search is the way out for large results, and clients split bulks.
- **Build cost.** `prost` and `protox` add compile time and size to an `edge` build only.
- **A schema pin goes stale.** The pinned upstream commit is recorded; a bump is a change with its own AC-4 run.
