# 0007: Registers by compare-and-swap, the Java's wire format and file lock

**Status:** Proposed, 2026-11-15.

## Context

Metadata the Java shell keeps in registers (index descriptors, shard commit registers, names) must be written by both runtimes during the strangler, so the Rust side needs the Java's `ConditionalBlobContainer` operations on every backend, with the same bytes and the same exclusion. On S3 the conditional write is the fence (AGENTS.md: CAS on a blob, no locks or leases). A filesystem has no conditional rename, so the Java's `FsBlobRegisters` serialises writers with a file lock, and a Rust writer on the same host must take the same lock or the two can lose each other's swaps.

## Decision

- **Wire format:** the Java's, on every backend: an 8-byte big-endian generation, then the value; first generation random in `[2^32, 2^62)` from the OS's secure source (`getrandom`, pinned).
- **S3 and the emulator:** ETags with `If-Match` and `If-None-Match: *` on PUT and `If-Match` on DELETE. A store remembers the last generation and ETag it read or wrote for up to 10,000 registers, as the Java's `knownVersions`, so a swap from that version is one PUT; without it, an 8-byte ranged GET and a PUT. A stale entry costs one refused PUT, never a wrong write.
- **Filesystem:** the Java's protocol exactly, the one lock this repository allows: 64 in-process stripes by the Java `String.hashCode` of the canonical path, and an exclusive whole-file lock on the stripe file, the one `FileChannel.lock` takes: a POSIX `fcntl` record lock on Unix (`rustix`, pinned; std's `File::lock` is `flock`, which does not exclude it), `LockFileEx` on Windows.

## Consequences

- Rust and Java processes share registers on S3 and on one host's filesystem; a test holds the lock from another process, and a JVM's lock was checked once by hand.
- Two new runtime dependencies in `oss-blob`: `getrandom` (already in the lock file) and `rustix` (Unix only).
- The filesystem lock is a correctness lock, but only for the single-host filesystem store; nothing on S3 uses a lock or lease.
- A POSIX record lock is released when any descriptor of the file closes in the process: only `fs/lock.rs` opens stripe files, one at a time per stripe, and releases the file before the stripe's mutex.
- The memory of versions is up to about 11 MiB per store at the cap (keys of at most 1,024 bytes), not yet reserved from the node budget; registers do not go through `PooledStore` yet. Both are for the first caller.
