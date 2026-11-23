# Mixed fleet takeover

Roadmap R8.7 ([plan](../../roadmap.md)). A Rust writer takes over a shard the Java shell wrote, and a shard it leaves is one the Java shell can open, so a fleet of both can lose either kind of node. Blob formats are shared ([ADR 0001](../../adr/0001-strangler-and-crate-layout.md)); this spec holds the end-to-end evidence over the shell's own recorded store (`fixtures/shell-blobs`: descriptor, commit register at term 2, a segment blob and a node pack). The janitor's dry-run default flips only on this and the fault suites ([ADR 0020](../../adr/0020-janitor-crate.md)).

## Acceptance criteria

- AC-1: A Rust writer whose head read names a dead Java owner at the commit's term takes the shard: the commit register is restored from the shell's blobs (a segment inside a node pack included), a document written is acknowledged at the next sequence number, and the term goes up by one.
- AC-2: The commit the Rust writer then publishes holds every document the Java commit held plus the new one, and still names the Java node pack and segment blob it did not rewrite, so neither is orphaned.
- AC-3: A janitor sweep of the shard after the publish, with dry run off, deletes no blob the new commit names.

## Test plan

`crates/node/tests/mixed_takeover.rs` loads the recorded blobs into the emulator, seeds the head the Java shell leaves for a dead owner, and runs the writer role of a second node over it.
