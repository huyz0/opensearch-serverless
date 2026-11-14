# oss-blob

The only path to the object store. Every S3 call goes through here and is counted.

## Owns
The S3 request kinds and their price classes (`src/op.rs`), the per-request meter (`src/meter.rs`), the blocking `BlobStore` trait ported from the shell's blob-container use (`src/store.rs`, with `src/chunk.rs` and `src/error.rs`), and the in-process S3 emulator behind the `emulator` feature (`src/emulator/`). The S3 client over `ureq` follows ([ADR 0005](../../docs/adr/0005-io-model.md)). It is the only crate allowed an S3 client dependency (`scripts/check-s3-access.py`).

## Depends on
`oss-types`.

## Entry points
`BlobStore` (get, put, create_if_absent, head, delete, list_prefix), `Emulator::new(capacity, seed)`, `Meter::record` (once per attempt, retries included), `S3Op::price_class`. Contract and criteria: [docs/specs/blob-store/SPEC.md](../../docs/specs/blob-store/SPEC.md). Adding a new kind of S3 call: [docs/how-to/add-an-s3-call.md](../../docs/how-to/add-an-s3-call.md).
