# 0020: The janitor is a crate beside the reader and the writer

**Status:** Accepted after independent review, 2026-11-19 (two rounds; the second round's four gaps, the derived LIST path, no timer sweep, the reservation gating concurrency and the shared term semantics, are answered above).

## Context

R8.4 ports the shell's `reconcile/JanitorPass`, `GarbageCollector`, the log cleanups, the tombstone and reclaim sweeps and `PublishedSweeps`. They delete, they run on a schedule over every shard of every index, and they need only the object store, the formats and the metadata plane: not the reader's search or the writer's indexing. Putting them in `oss-writer` would make the writer's crate hold what a node running only the janitor, or the fault suites, must link; putting them in `oss-meta` would give the metadata crate a role (a pass over shards, pins from point in time and snapshots) that ADR 0008 kept out of it. The Java keeps them in one package, `reconcile`, apart from both roles.

## Decision

- **A new crate `oss-janitor`** at layer 3, beside `oss-reader` and `oss-writer`, depending on `oss-types`, `oss-blob`, `oss-format`, `oss-cluster` (leases, shard heads) and `oss-meta` (descriptors), and never on the reader or the writer. It is added to `LAYERS` in `check-layering.py` and to the architecture table; `oss-node` wires it at layer 5.
- **Deletion rules live there once**: a blob goes only when it is in a term older than the published manifest's, nothing names it (the manifest, a point in time, a snapshot), and two consecutive sweeps saw it unreferenced. The crate ships no "no pins" answer; the pins come from the point-in-time and snapshot work.
- **Every pass starts as a dry run**: a node setting, `serverless.janitor.dry_run`, default on, flipped when the fault suites prove no live blob is ever counted.

## Cost, memory, fencing and the Java

- **What triggers a sweep, and what it costs.** A sweep is of one shard and is started by the pass that owns the trigger, never by a timer over every index: a node sweeps a shard it holds when a commit it published dropped a file or the pins changed (the Java's per-node sweep gate), and no sweep is scheduled over the whole deployment: enumerating every index on a timer is O(indices) and is not built. A pass that must visit shards it did not publish (a dead node's) takes them from the takeover path, which already enumerates by lease, not by index. A shard whose pins keep changing is swept once per trigger, so its cost is one sweep per change, and a hibernated node triggers none. An idle index with nothing dropped is never swept: zero requests. A sweep is one register read, one LIST per 1,000 keys under that one shard's derived path, `segments/<index>#<uuid>#<shard>/` (`ShardRef::base_path`, never a scan of the bucket; `// LIST:`; the listing is how unreferenced blobs are found, and it scales with that shard's blobs and nothing else) and one DELETE per 1,000 blobs deleted. Each pass lands with its own row in the request-budget table and a test that an untriggered index makes none.
- **Memory.** A sweep holds one manifest buffer (a pooled one, the caller's), the manifest's referenced keys (at most the manifest's file limit) and the candidates it is watching, capped per shard by `Config::max_watched` (default 100,000 keys, about 100 MiB at the longest key); blobs past the cap are judged by later sweeps as the watched ones go. The pass reserves the sweep's worst case from the node's pool before it starts, and that reservation gates concurrency: at the limit a sweep is deferred to the next trigger, never refused to a client.
- **Fencing and durability.** A delete needs no lock, lease or compare-and-swap, because it applies only to blobs in a term strictly older than the live manifest's (`CommitManifest::term`, the publishing writer's shard-head term, which the Java's `CommitManifest` writes and its `SegmentPublisher` fences on identically; the formats are the same bytes): a writer of an older term can no longer swap the register (the publisher's term check), and a publish in flight writes only its own, live term's container. What the manifest or a pin names is never deleted. The "seen twice" memory is in the process: a restart or a new owner forgets it, which delays a deletion and never causes one. No extra PUT is made to keep it.
- **The Java on the same bucket.** Formats are shared, so a Rust sweep must see every pin a Java node holds. The real `Pins` reads the point-in-time and snapshot records in the Java's formats; until it exists nothing can wire a sweep, and when it does the pass runs as a dry run.
- **The default flips on evidence.** Changing `serverless.janitor.dry_run` to off is a default change under the switch rule: it needs the fault suites (R8.6) and the mixed-fleet runs (R8.7) to show no live blob is ever counted, recorded in the spec's `VERIFIED.md`, in a PR of its own.

## Consequences

- The writer and the reader stay free of the deletion code; the janitor can be tested, mutated and run on its own.
- A new workspace crate: high-risk, per the how-to. It adds no request on any path that exists today, no memory beyond what a pass holds (bounded by the unreferenced blobs it is watching), and one switch.
- Rules out: the janitor in the writer, a "no pins" default, and a pass that deletes before its dry run has been trusted.
