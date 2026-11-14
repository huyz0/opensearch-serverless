# Add an S3 call

Every S3 request costs money and goes through `oss-blob`.

1. **First ask whether you need it.** Can the key be derived from data you already hold? Can the information ride in an upload that already happens? A LIST costs as much as a PUT; never add a PUT to save the same number of LISTs or fewer.
2. Add the operation to the store trait in `crates/blob` (not in the calling crate). If it is a new request kind, add it to `S3Op` with its price class in `crates/blob/src/op.rs`.
3. The implementation calls `Meter::record` once per attempt, retries included.
4. A LIST call site carries a `// LIST:` comment saying why nothing cheaper works.
5. Write a test against the in-process emulator asserting the exact request counts of the operation that uses it (for example, "a refresh issues one PUT and no LIST").
6. State the request cost per operation in the PR description.

**Checks:** `scripts/check-s3-access.py`; the count assertions in your tests.
