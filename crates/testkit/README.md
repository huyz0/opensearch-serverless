# oss-testkit

Test-only support shared by every crate's tests: a counting global allocator that records the peak, and helpers to assert peak memory under load. A dev-dependency only; never a normal dependency.

## Owns
Test-only helpers: a counting allocator for peak-memory assertions.

## Depends on
Nothing in the workspace.

## Entry points
`src/lib.rs`. The counting allocator and peak assertions land in stage 1.
