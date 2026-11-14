# oss-blob

The only path to the object store. Every S3 call goes through here and is counted.

## Owns
The S3 request kinds and their price classes (`src/op.rs`), the per-request meter (`src/meter.rs`), and, once the runtime is chosen, the store trait and its RustFS-tested implementation. It is the only crate allowed an S3 client dependency (`scripts/check-s3-access.py`).

## Depends on
`oss-types`.

## Entry points
`Meter::record` (call once per attempt, retries included), `S3Op::price_class`. Adding a new kind of S3 call: [docs/how-to/add-an-s3-call.md](../../docs/how-to/add-an-s3-call.md).
