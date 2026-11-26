# Blob store trait and in-process S3 emulator

## Serves

Roadmap stage 1 ("the store trait in `oss-blob`", "an in-process S3 emulator"), invariants I-1 to I-3 and I-15, ADRs [0004](../../adr/0004-bounded-memory-and-streaming.md) and [0005](../../adr/0005-io-model.md).

## Change

- `oss-blob` gains the `BlobStore` trait, ported from the shell's blob-container use through `MeteredBlobStore` (huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/store/MeteredBlobStore.java@bf347adb`): ranged or whole GET into a caller-provided buffer and chunk sink, PUT from a chunk source, conditional create (`If-None-Match: *`), HEAD, DELETE, and a paginated LIST capped at 1,000 keys a page.
- `Meter` gains the Java counters it lacked: bytes read, bytes written, and errors.
- An in-process emulator behind the `emulator` Cargo feature, with bounded total bytes, per-operation latency, failures and throttling, a deterministic seed, and exact request counts.
- Not in this change: the real S3 client (ADR 0005 puts it in its own change), retries, the buffer pool (`oss-mem`, built in parallel; callers pass `&mut [u8]` so it plugs in), the Java's per-thread request attribution, register compare-and-swap (`If-Match`) and ETags, batch delete (`DeleteObjects`, up to 1,000 keys a request: until it lands, deleting n objects costs n free DELETE requests, which the garbage collector must not ship with), mid-body faults in the emulator, and the peak-memory assertion (after `oss-mem` lands).

## Acceptance criteria

- AC-1: each operation records exactly one attempt of its request kind: a whole GET is one `Get`, a ranged GET one `GetRange`, a PUT or conditional create one `Put`, a HEAD one `Head`, a DELETE one `Delete`, a LIST page one `List`, and nothing else is recorded.
- AC-2: a failed attempt (injected failure, throttle, missing key, refused create, invalid range) is still recorded as one attempt of its kind and once in the error count.
- AC-3: a GET streams: an object of 4 MiB read through a 64 KiB caller buffer reaches the sink as 64 chunks, none larger than the buffer, and the bytes match what was written.
- AC-4: a ranged GET returns exactly the requested bytes, truncated at the object's end; a range starting at or past the end fails with `InvalidRange`.
- AC-5: a conditional create of a key that exists returns `AlreadyExists` and leaves the stored bytes unchanged, even when the emulator is near capacity; of eight threads racing to create one key, all past any pre-check before any commits, exactly one gets `Created`.
- AC-6: a PUT whose source fails or yields a length other than the one it declared stores nothing visible; the previous object, if any, is unchanged.
- AC-7: a LIST page returns at most `min(max_keys, 1000)` keys in lexicographic order after `start_after`, says whether more remain, and listing 2,500 keys page by page visits each exactly once in 3 LIST requests.
- AC-8: the emulator refuses a PUT that would take its stored bytes over the configured capacity, with `CapacityExceeded`, and stores nothing.
- AC-9: with the same seed, the n-th request of a kind gets the same injected failure or throttle whatever other kinds do; scripted faults (`fail the next n`) hit exactly the next n requests of that kind.
- AC-13: a lost response (`lose_response_next`) commits the write and answers `Unavailable`; a retried conditional create of that key, its body rewound, then gets `AlreadyExists`.
- AC-10: injected latency delays each request of that kind by at least its base.
- AC-11: an empty caller buffer, an empty key, a key over 1,024 bytes, or `max_keys` of 0 fails with `InvalidArgument` before any request is recorded.
- AC-14: a request the test held (`hold_next`) is counted when it arrives and does nothing until released; the n-th request of the kind takes the n-th hold; released out of arrival order it ends as the release says (lands, fails, or lands and then answers `Unavailable`).
- AC-12: every request-path allocation in `crates/blob/src` states its bound, and nothing outside the emulator (which holds objects in memory by design, up to its capacity) reads a whole object into memory: `scripts/check-alloc.py` and the clippy `disallowed-methods` list pass.

## Test plan

Each criterion is a test in `crates/blob/tests/` against the emulator, written before the code it checks:

- AC-1, AC-2: count assertions per operation; they catch a store that records twice or forgets an error path.
- AC-3: a sink that records chunk sizes; catches a store that buffers whole objects.
- AC-4, AC-7: proptest over ranges and key sets; catches off-by-one at the object end and page boundaries.
- AC-5: a threaded race whose bodies wait on a barrier; catches a check-then-insert outside the commit lock.
- AC-13: a lost response then a retry; catches an emulator that hides committed-but-failed writes from fencing callers.
- AC-6, AC-8: failing and short sources, a small capacity; catch partial writes made visible.
- AC-9, AC-10: two emulators with one seed compared request by request; elapsed time against a base latency.
- AC-11: invalid inputs with the meter checked at zero.
- AC-14: three PUTs held and released in the order third, first, second, each with a different outcome; the stored objects are checked after each release.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | Depth |
|---|---|---|---|---|
| `get` (whole or ranged) | 0 | 1 | 0 | 1 |
| `head` | 0 | 1 | 0 | 1 |
| `put`, `create_if_absent` | 1 | 0 | 0 | 1 |
| `delete` | 0 | 0 | 0 | 1 (free) |
| `list_prefix` page | 1 (LIST) | 0 | 1 | 1 per page |

No retries yet, so one attempt per call.

## Switch

None: a new trait and a test-only emulator feature; no behaviour of a running node changes.

## Risks

- Emulator semantics drift from S3 (for example, S3 answers a racing conditional create with 409 `ConditionalRequestConflict`, which the trait reports as `BlobError::Conflict` but the emulator never returns). The S3 client should pass the same tests, generic over `&dyn BlobStore`, plus RustFS runs on the wire.
- The trait shape may not fit the buffer pool's handle type; it takes plain `&mut [u8]` so a pooled buffer derefs into it.
