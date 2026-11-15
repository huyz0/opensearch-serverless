# Pooled blob store

## Serves

Roadmap stage 1 wiring; invariants I-15, I-16, I-17; ADR 0004.

## Change

`oss-blob` gains `PooledStore`, which streams every body through a buffer checked out of the node's `BufferPool` (`oss-mem`) and returns it on every path. When the pool is empty the call fails with the new `BlobError::Busy` before any request is sent. Does not change the `BlobStore` trait or the emulator.

## Acceptance criteria

- AC-1: get, put and create_if_absent round-trip through the pool and give the buffer back, on success and on error.
- AC-2: with every buffer checked out, each call answers `Busy` and the meter records no attempt.
- AC-3: a 16 MiB object is written and read through one 64 KiB buffer; the read allocates nothing that grows with the object, and the write holds no second copy of the body.

## Test plan

AC-1 and AC-2 are unit tests in `pooled.rs`. AC-3 is a counting-allocator test in its own binary, seen failing when the emulator's GET was made to copy the object.

## S3 request budget

Unchanged: one attempt per call; zero when `Busy`.

## Switch

None: no behaviour existed before.

## Risks

A pool sized too small turns load into `Busy` answers; the pool size is a node setting in a later change.
