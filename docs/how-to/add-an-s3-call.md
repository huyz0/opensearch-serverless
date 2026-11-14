# Add an S3 call

Every S3 request costs money and goes through `oss-blob`.

1. **First ask whether you need it.** Can the key be derived from data you already hold? Can the information ride in an upload that already happens? A LIST costs as much as a PUT; never add a PUT to save the same number of LISTs or fewer.
2. Use the `BlobStore` trait in `crates/blob/src/store.rs`; if nothing there fits, add the operation to the trait (not in the calling crate). A new request kind goes in `S3Op` with its price class in `crates/blob/src/op.rs`. Calls block ([ADR 0005](../adr/0005-io-model.md)).
3. Every implementation (the emulator in `crates/blob/src/emulator/`, the S3 client) calls `Meter::record` once per attempt, retries included, and `Meter::record_error` for each attempt that fails. Bodies stream through a caller-provided `&mut [u8]` into a `ChunkSink` or out of a `ChunkSource`; nothing collects a whole object into a `Vec<u8>` (ADR 0004). Argument checks come before the attempt and record nothing.
4. A LIST call site carries a `// LIST:` comment saying why nothing cheaper works.
5. Write a test against the in-process emulator (`oss-blob` with the `emulator` feature, as a dev-dependency) asserting the exact request counts of the operation that uses it (for example, "a refresh issues one PUT and no LIST").
   Use `Emulator::fail_next`, `throttle_next`, `lose_response_next`, `set_faults` and `set_latency` to test the failure and slow paths; with one seed the n-th request of each kind gets the same fault, though which thread's request is n-th depends on scheduling.
6. State the request cost per operation in the PR description.

**Checks:** `scripts/check-s3-access.py`; the count assertions in your tests.
