# oss-mem

The node's memory budget: named pools, reservations taken before allocation, and a pool of fixed-size I/O buffers returned on drop. Every other crate gets memory through here ([ADR 0004](../../docs/adr/0004-bounded-memory-and-streaming.md)).

## Owns
The node memory budget, pools, reservations and fixed-size buffer pool.

## Depends on
Nothing in the workspace.

## Entry points
`src/lib.rs`. The budget, reservation and buffer-pool types land in stage 1.
