# oss-mem

The node's memory budget: named pools (request bodies, query execution, write buffers, page cache, transport, open indexes), reservations taken before allocation, and a pool of fixed-size I/O buffers returned on drop. Every other crate gets memory through here ([ADR 0004](../../docs/adr/0004-bounded-memory-and-streaming.md)).

## Owns
The node memory budget, pools, reservations and fixed-size buffer pool.

## Depends on
Nothing in the workspace.

## Entry points
`src/lib.rs` re-exports them:

- `MemoryBudget::new(&BudgetConfig)`, then `try_reserve(Pool, bytes)` returning a `Reservation` (released on drop; `try_grow`, `shrink`) or `Exhausted` (map to `Exhausted::HTTP_STATUS`, 429, or back off).
- `BufferPool::new(&budget, pool, size, count)` preallocates and charges `size * count`; `try_checkout()` returns a `PooledBuffer` that goes back on drop. `SMALL_BUFFER` is 64 KiB, `LARGE_BUFFER` 1 MiB.
