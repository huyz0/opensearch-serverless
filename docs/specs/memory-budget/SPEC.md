# Memory budget and test allocator

## Serves

[ADR 0004](../../adr/0004-bounded-memory-and-streaming.md), decisions 1 and 2, and invariants I-16 and I-17 in [invariants.md](../../invariants.md).

## Change

- `oss-mem` gains the node `MemoryBudget`: named pools (`Pool`), each sized in bytes, whose sizes sum to at most the node total. `try_reserve(pool, bytes)` returns an RAII `Reservation` released on drop, or a typed `Exhausted` error the caller maps to 429 (`Exhausted::HTTP_STATUS`) or to backpressure. A reservation can grow (fallibly) and shrink.
- `oss-mem` gains `BufferPool`: `count` buffers of one fixed size (`SMALL_BUFFER` 64 KiB, `LARGE_BUFFER` 1 MiB, or any size), allocated once at construction and charged to a pool for the pool's lifetime. `try_checkout` returns a guard that returns its buffer on drop, or `Exhausted` when every buffer is out. Checkout and return never call the global allocator.
- `oss-testkit` gains a counting global allocator (the vetted `cap` crate, pinned, so this repo holds no `unsafe`), a `PeakWindow` measuring the high-water mark from a start point, and `assert_peak_below`. It stays a dev-dependency only.

Not in this change: wiring the budget into the node, settings for pool sizes, arenas for per-request scratch, waiting for a reservation (callers apply backpressure themselves).

## Acceptance criteria

- AC-1: a reservation of exactly a pool's free bytes succeeds; one byte more fails with `Exhausted` naming the pool, the bytes requested, the bytes in use and the limit.
- AC-2: dropping a reservation returns its bytes to its pool; pools are independent (exhausting one leaves the others' free bytes unchanged).
- AC-3: a budget whose pools sum to the node total is accepted; one byte more is refused with a config error, as is a sum that overflows.
- AC-4: `Reservation::try_grow` to exactly the pool limit succeeds and one byte more fails, leaving the reservation unchanged; `shrink` releases bytes immediately.
- AC-5: a `BufferPool` of `count` buffers charges `size * count` bytes to its pool at construction, fails with `Exhausted` when the pool cannot hold that, and returns the bytes when the last handle and guard drop.
- AC-6: `count` checkouts succeed and checkout `count + 1` fails with `Exhausted`; returning one guard lets the next checkout succeed.
- AC-7: 16 threads each doing 10,000 checkouts and reservations against an 8-buffer pool never hold more than 8 buffers or exceed a pool limit, and every buffer and byte is back when they finish.
- AC-8: steady-state checkout and return make zero global allocations, measured by the counting allocator over 10,000 cycles.
- AC-9: `assert_peak_below` passes when the peak since the window opened is at most the bound and panics when it is one byte over.
- AC-10: a sustained load test (8 threads, 2,000 requests each, every request reserving before it allocates and backing off on `Exhausted`) keeps the process's peak heap growth at or under the configured budget plus a fixed harness allowance, and at least one request is refused, so the bound was exercised.
- AC-11: no crate lists `oss-testkit` under `[dependencies]` (`scripts/check-layering.py`).

## Test plan

- AC-1, AC-4, AC-6: limit and limit+1 tests; they fail if the comparison is `<` instead of `<=` or the check is skipped.
- AC-2, AC-5: reservations and pools dropped, then the free bytes read back; they fail if `Drop` is missing or releases the wrong pool.
- AC-3: config tests at the total and total+1, and at `usize::MAX` pools.
- AC-7: a stress test with a shared high-water counter; fails on a lost update in the CAS loop or a double return.
- AC-8, AC-9, AC-10: integration tests in `oss-mem/tests` and `oss-testkit/tests`, each its own binary with `#[global_allocator]` so no other test shares the counter; AC-10 is evidence for I-17.

## S3 request budget

Unchanged: no S3 calls.

## Switch

None: a library with no default behaviour change; nothing uses it yet.

## Risks

- The peak is process-wide and cannot be reset (`cap` keeps a running maximum), so a window reports `max - baseline`. If the process peaked before the window opened, the reading is too high, never too low: a false failure, not a false pass.
- A caller that allocates without reserving is invisible to the budget; `// ALLOC:` comments and the peak test are what catch it.
