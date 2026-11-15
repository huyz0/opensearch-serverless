# oss-blob

The only path to the object store. Every S3 call goes through here and is counted.

## Owns
The S3 request kinds and their price classes (`src/op.rs`), the per-request meter (`src/meter.rs`), the blocking `BlobStore` trait ported from the shell's blob-container use (`src/store.rs`, with `src/chunk.rs` and `src/error.rs`), the in-process S3 emulator behind the `emulator` feature (`src/emulator/`) with its S3 HTTP front end for testing the client (`src/emulator/http/`), and the S3 client (`src/s3/`): `ureq`, SigV4 written here, path-style addressing, and retries that record every attempt ([ADR 0005](../../docs/adr/0005-io-model.md)). It is the only crate allowed an S3 client dependency (`scripts/check-s3-access.py`).

## Depends on
`oss-types`, `oss-mem`; externally `ureq` (rustls), `sha2` and `hmac`, each pinned at an exact version.

## Entry points
`PooledStore` streams bodies through the node's buffer pool and answers `BlobError::Busy` when it is empty; request paths use it rather than passing their own buffers. `BlobStore` (get, put, create_if_absent, head, delete, list_prefix), `S3Store::new(S3Config::new(endpoint, bucket, region, Credentials::new(..)))`, `Emulator::new(capacity, seed)`, `emulator::http::S3Server::start` for client tests, `Meter::record` (once per attempt, retries included), `S3Op::price_class`. Contract and criteria: [docs/specs/blob-store/SPEC.md](../../docs/specs/blob-store/SPEC.md), and for the S3 client [docs/specs/s3-client/SPEC.md](../../docs/specs/s3-client/SPEC.md). `tests/rustfs.rs` runs the client against a RustFS binary when `RUSTFS_BIN` names one. Adding a new kind of S3 call: [docs/how-to/add-an-s3-call.md](../../docs/how-to/add-an-s3-call.md).
