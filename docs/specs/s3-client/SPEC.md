# S3 client

## Serves

Roadmap stage 1 ("the S3 client"), invariants I-1 to I-3 and I-15, ADR [0005](../../adr/0005-io-model.md) decision 3, ADR [0004](../../adr/0004-bounded-memory-and-streaming.md).

## Change

- `oss-blob` gains `S3Store`, a blocking `BlobStore` over S3's REST API: `ureq` (pinned `=3.5.2`, rustls) for HTTP, AWS Signature V4 written here (`s3/sigv4.rs`), path-style addressing (`http(s)://host/bucket/key`), and a retry policy that records every attempt in the `Meter`.
- Requests: GET (whole, or `Range: bytes=a-b`), PUT with `Content-Length` and an `UNSIGNED-PAYLOAD` body streamed from the caller's buffer, conditional create with `If-None-Match: *`, HEAD, DELETE, and one `ListObjectsV2` page (`list-type=2`, at most 1,000 keys) parsed as it streams.
- Status mapping, ported from the shell's `S3BlobContainer.isConditionFailure` (huyz0/OpenSearch `plugins/repository-s3/src/main/java/org/opensearch/repositories/s3/S3BlobContainer.java@49f238ed`): 412, or 409 with code `PreconditionFailed`, is `AlreadyExists`; any other 409 (`ConditionalRequestConflict`) is `Conflict` and is retried, since the precondition's outcome is unknown.
- `BlobError` gains `Denied` (401, 403) and `Unexpected(status)` and `Protocol(reason)` for responses the client cannot use.
- The emulator gains a minimal S3 HTTP front end (`emulator::http::S3Server`, `emulator` feature only) that verifies SigV4 on every request, refuses a PUT without `Content-Length`, and can script error statuses and cut bodies short.
- Not in this change: batch `DeleteObjects`, ETags and `If-Match`, multipart upload, virtual-hosted addressing, resuming a GET part-way (the shell's `S3RetryingInputStream` does; without an ETag a resumed read could mix two versions of a key), per-thread attribution, and wiring the store into a node.

## Acceptance criteria

- AC-1: the signer reproduces the canonical request, string to sign and `Authorization` header of every vector from AWS's published SigV4 test suite kept under `crates/blob/tests/sigv4/`, and the `Authorization` header of AWS's published S3 example (GET Object with `Range: bytes=0-9`).
- AC-2: on success each call records exactly one attempt of its kind on the client's meter and the server serves exactly one request: GET `Get`, ranged GET `GetRange`, PUT and conditional create `Put`, HEAD `Head`, DELETE `Delete`, LIST page `List`.
- AC-3: a ranged GET sends `Range: bytes=a-b` and returns exactly those bytes, truncated at the object's end; a range at or past the end is `InvalidRange`, a missing key `NotFound`, each in one attempt.
- AC-4: a PUT always sends `Content-Length` and never chunked encoding (the test server answers 411 otherwise); a source that yields more or fewer bytes than it declared fails the call and the previous object is unchanged.
- AC-5: a conditional create sends `If-None-Match: *`; a taken key is `AlreadyExists` (412, or 409 `PreconditionFailed`) in one attempt; a 409 `ConditionalRequestConflict` is retried; of eight threads racing to create one key over HTTP, exactly one gets `Created`.
- AC-6: HEAD of a missing key is `None` with no error counted; DELETE of a missing key succeeds.
- AC-7: a LIST page visits at most `min(max_keys, 1000)` keys in byte order after `start_after`, with sizes, decodes XML-escaped keys, reports truncation, and 2,500 keys are listed in exactly 3 LIST requests; a malformed or over-long element is `Protocol`.
- AC-8: 500, 502, 503, 504, 429 and connection failures are retried up to `max_attempts`, every attempt recorded once and every failed one once as an error; 401, 403, 404, 412, 416 and other 4xx are not retried; each pause lies in `[0, min(cap, base * 2^retry)]`.
- AC-9: a GET is retried only before its first chunk reaches the sink: a body cut part-way is `Unavailable` after one attempt and the sink saw each byte at most once. A PUT is retried only after `rewind` succeeds; a source that cannot rewind gets one attempt.
- AC-10: a lost response to a conditional create (committed, then 500) is retried and answers `AlreadyExists`, two attempts recorded; the trait documents that `AlreadyExists` after a retry may be the caller's own write, as the emulator already behaves.
- AC-11: a request signed with the wrong secret is `Denied` in one attempt.
- AC-12: an empty key, a key over 1,024 bytes, an empty buffer, a zero-length range or `max_keys` of 0 fails with `InvalidArgument` before any request.
- AC-13: a 16 MiB object is written and read over HTTP through one 64 KiB pooled buffer: the PUT's peak heap stays under one copy of the object (the emulator's) plus a fixed overhead, and the GET's under the fixed overhead.
- AC-14: with `RUSTFS_BIN` set, the round-trip, conditional create, ranged GET, HEAD, DELETE and LIST scenario passes against a RustFS binary on loopback; without it the test says it skipped.
- AC-15: no test or script names a real AWS endpoint (`scripts/check-test-endpoints.py`), every request-path allocation states its bound (`scripts/check-alloc.py`), and the one LIST call site carries `// LIST:` (`scripts/check-s3-access.py`).
- AC-16: with 50 ms injected on every GET, 192 threads each fetching in turn (the ADR's 256-thread cap less the front end's headroom) finish 4 rounds within 4 times the parallel time of 200 ms, and a HEAD issued meanwhile takes a median under 25 ms: blocking fetches up to the cap neither queue nor starve other requests.

## Test plan

- AC-1: `tests/s3/sigv4.rs` parses each `.req`, signs it, and compares the `.creq`, `.sts` and `.authz` files byte for byte; a sort or trim slip changes the signature.
- AC-2 to AC-12: `tests/s3/` against `S3Server` on loopback, asserting client and server counts, each seen failing against a planted defect (no `If-None-Match`, every 409 taken as lost, a retry after bytes reached the sink, no length check before the last byte, a server that skips the signature check, an off-by-one range, an unrecorded failed attempt, unsorted signed headers). `tests/s3/server.rs` and `tests/s3/protocol.rs` cover the front end's refusals and the client's handling of responses no correct S3 sends.
- AC-8, AC-9: scripted statuses and cut bodies (`S3Server::answer_next`, `cut_body_next`), the emulator's `fail_next`, and a closed port for connection failures; a pure test of the backoff bound.
- AC-13: `tests/s3_streaming_peak.rs`, its own binary because the counting allocator is process-wide.
- AC-14: `tests/rustfs.rs`, with `RUSTFS_BIN` set to a RustFS binary.
- AC-16: `tests/s3/load.rs`, the emulator's `set_latency` on GET, the client's pool sized to the threads.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | Depth |
|---|---|---|---|---|
| `get` (whole or ranged) | 0 | 1 per attempt | 0 | 1 |
| `head` | 0 | 1 per attempt | 0 | 1 |
| `put`, `create_if_absent` | 1 per attempt | 0 | 0 | 1 |
| `delete` | 0 | 0 | 0 | 1 (free) |
| `list_prefix` page | 1 per attempt | 0 | 1 per attempt | 1 |

At most `max_attempts` (default 4) attempts per call, each recorded; no call issues a request of another kind.

## Switch

None: a new store implementation nothing uses yet; no running node's behaviour changes. The retry policy, timeouts and connection pool size are `S3Config` fields.

## Risks

- A fenced writer that treats every `AlreadyExists` as "I lost" misreads a create whose first response was lost. The trait now says so; a distinct outcome would change the fencing contract and is the owner's call.
- A signer that matches the vectors but not S3's path or query canonicalisation: the test server re-derives the canonical request from the bytes on the wire, and the RustFS run checks a real verifier.
- RustFS never answers 409 (seen by the shell's RustFS run), so the 409 paths are tested only against the scripted server.
- `UNSIGNED-PAYLOAD` relies on TLS for body integrity; a plain-`http` endpoint is for loopback tests only.
