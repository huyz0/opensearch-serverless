# 0004: Bounded memory and streaming on every path

**Status:** Accepted by the owner, 2026-11-14.

## Context

A node holds thousands of shards for many tenants on a fixed amount of memory. One tenant's large bulk, wide aggregation or slow client must not take memory from the others or kill the process. Java's answer (heap, circuit breakers, GC) bounds memory loosely and late. Rust lets memory be bounded by construction: sizes are known, allocation is explicit, and ownership makes a buffer's lifetime visible.

## Decision

1. **Every node has a memory budget**, split into named pools (request bodies, query execution, write buffers, page cache, transport, and open indexes since [ADR 0014](0014-open-indexes-pool.md)). A component reserves from its pool before it allocates. When a reservation fails, the request is refused with 429 or held by backpressure; it never runs the process out of memory.
2. **Fixed-size buffers from pools.** I/O uses fixed-size buffers (a few sizes, for example 64 KiB and 1 MiB) taken from a per-node pool and returned on drop. Per-request scratch uses an arena reset when the request ends. Steady-state request paths do not call the global allocator.
3. **Streaming end to end.** REST request and response bodies, node-to-node transport, writes to and reads from the object store are processed in bounded chunks. Nothing reads a whole body, blob or result set into memory. A bulk request is parsed by a streaming parser that hands documents on as they arrive.
4. **Bounded channels and queues only.** Every queue has a capacity; a full queue pushes back to its producer.
5. **No unbounded growth on request paths.** A `Vec`, `String` or map that grows with input has a stated maximum, checked before it grows, and a test at the limit and one past it.

## Enforcement

- `clippy.toml` disallows whole-buffer reads (`read_to_end`, `read_to_string`, `std::fs::read`) and unbounded channels; a test may allow them locally.
- `scripts/check-alloc.py`: in request-path crates (`blob`, `reader`, `writer`, `coordinator`), every allocation site (`Vec::new`, `vec!`, `String::new`, `format!`, `to_vec`, `to_string`, `collect`, `Box::new`, `with_capacity`) carries an `// ALLOC:` comment naming its bound, or it does not compile into a request path. Test modules are exempt.
- Tests assert peak memory under load: a counting allocator in the test harness records the high-water mark, and a load test asserts it stays under the configured budget. The allocator is the pinned `cap` crate (test-only), so the repository holds no `unsafe` of its own. Its running maximum cannot be reset, so a reading is the maximum minus the bytes in use at the start: it can read high, never low. Each peak test is its own test binary, since the counters are process-wide.
- Invariants I-15 to I-17 in [invariants.md](../invariants.md).

## Consequences

- Overload degrades into 429s and slower clients, never a crash.
- More code up front: pools, budgets and streaming parsers instead of `Vec<u8>` and `serde_json::from_slice`. Porting Java that materialises (whole-body parsing, collected hit lists) needs redesign at those points; the port map notes each one.
- `// ALLOC:` comments make every allocation on a request path visible in review, at the cost of some noise.
