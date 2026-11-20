# One store shared by every role of a node

## Serves

Parity plan R7.6, node assembly: the reader wants its store inside a `PooledStore<S>` by value, while the writer, the point-in-time store and the janitor hold `Arc<dyn BlobStore>`. A node running several roles has one store (one connection pool, one meter), so the roles must reach it through one handle.

## Change

`oss-blob` gains `Shared`: a `BlobStore` that forwards every call, `delete_many` and `validate_key` included, to an `Arc<dyn BlobStore>`, and answers that store's meter. Cloning it shares the store.

## Acceptance criteria

- AC-1: what is written through a `Shared` reads back through the store it wraps and the other way round, for objects and for registers (create, read, swap, delete).
- AC-2: a `Shared` makes exactly the requests the wrapped store would: the meter it answers is the wrapped store's, a batch delete is one request a batch, and a listing is one page.
- AC-3: `validate_key` is the wrapped store's rule, not the default.

## Test plan

`crates/blob/tests/shared.rs` over the emulator.

## S3 request budget

None of its own: each call is one call of the wrapped store.
