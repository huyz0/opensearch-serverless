# Mixed fleet takeover

Roadmap R8.7 ([plan](../../roadmap.md)). A Rust writer takes over a shard the Java shell wrote, and a shard it leaves is one the Java shell can open, so a fleet of both can lose either kind of node. Blob formats are shared ([ADR 0001](../../adr/0001-strangler-and-crate-layout.md)); this spec holds the end-to-end evidence over the shell's own recorded store (`fixtures/shell-blobs`: descriptor, commit register at term 2, a segment blob and a node pack). The janitor's dry-run default flips only on this and the fault suites ([ADR 0020](../../adr/0020-janitor-crate.md)).

## Acceptance criteria

- AC-1: A Rust writer whose head read names a dead Java owner at the commit's term takes the shard: the commit register is restored from the shell's blobs (a segment inside a node pack included), a document written is acknowledged at the next sequence number, and the term goes up by one.
- AC-2: The commit the Rust writer then publishes holds every document the Java commit held plus the new one, and still names the Java node pack and segment blob it did not rewrite, so neither is orphaned.
- AC-3: A janitor sweep of the shard after the publish, with dry run off, deletes no blob the new commit names.
- AC-4: The shell's own classes open the commit the Rust writer published: `CommitManifest.fromStream` and `FileLocation.parse` read it, the files kept in the Java node pack and in the Java segment blob as well as the Rust blob, and a Lucene reader through the soft-deletes wrapper counts the Java's documents and the Rust writer's. The Rust reader counts the same.
- AC-5: The shell's `ShardHeadStore` reads the head the Rust take left (term 3, the Rust owner, the Java node among the writers) and, once the Rust node is dead by lease and oracle, acquires the shard at term 4 naming the Rust node among the writers.

## Test plan

`crates/node/tests/mixed_takeover.rs` loads the recorded blobs into the emulator, seeds the head the Java shell leaves for a dead owner, and runs the writer role of a second node over it. AC-4 and AC-5: `fixtures/mixed-takeover` holds that store as the Rust test left it, the shell's classes' output over it (`java.txt`, from `JavaTakesBack.java`), and `mixed_java_reads.rs` has the Rust reader count the same.
