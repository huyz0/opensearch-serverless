# Architecture decision records

One file per decision: `NNNN-short-title.md`, numbered in order, never renumbered or deleted. A decision that is overturned stays, with its status changed to `Superseded by NNNN`.

**When one is required:** a PR that touches blob formats, durability or fencing, a new workspace crate, public API, the S3 request pattern, a default, or security links an ADR or adds one (`scripts/check-pr.py`). A third-party dependency change alone does not ([ADR 0002](0002-runtime-dependencies.md)). A new ADR starts as **Proposed**.

**Who accepts:** the thread that wrote it, after an independent review (a separate review thread or subagent given the ADR and [principles.md](../principles.md), not the author's reasoning) finds that it keeps the owner's core requirements: S3 request cost, bounded memory, scalability (requests scale with nodes and bytes, idle indices free), durability and fencing, and OpenSearch compatibility. The status line names the review: `Accepted after independent review, <date>`. An ADR that trades any of those away, or that the review cannot clear, stays Proposed and goes to the owner. The owner's rule, 2026-11-16.

Each record has: **Status** (Proposed, Accepted, Superseded), **Context** (the forces, with numbers where they exist), **Decision**, **Consequences** (what gets easier, what gets harder, what it rules out).

| ADR | Title | Status |
|---|---|---|
| [0001](0001-strangler-and-crate-layout.md) | Strangler beside the Java shell, layered crates | Accepted |
| [0002](0002-runtime-dependencies.md) | lucene-rust is the only code dependency; hsearch and pstore are references | Accepted |
| [0003](0003-apache-2-licence.md) | Apache License 2.0 | Accepted |
| [0004](0004-bounded-memory-and-streaming.md) | Bounded memory and streaming on every path | Accepted |
| [0005](0005-io-model.md) | Blocking object-store I/O, and a small S3 client of our own | Accepted |
| [0006](0006-http-stack.md) | A small blocking HTTP/1.1 server of our own, on a bounded worker pool | Accepted |
| [0007](0007-registers.md) | Registers by compare-and-swap, the Java's wire format and file lock | Accepted |
| [0008](0008-metadata-crate.md) | A metadata crate between the stores and the roles | Accepted |
| [0009](0009-unsafe-when-guaranteed-safe.md) | `unsafe` in any crate, only when guaranteed safe | Accepted |
| [0010](0010-one-home-per-rule.md) | One home per rule, and a harness that checks itself | Accepted |
| [0011](0011-cluster-config-flat-form.md) | The cluster config register written in the flat settings form | Accepted |
| [0012](0012-index-deletes-and-reclaim.md) | Index deletes finish through a reclaim queue, and purge a shard by listing it | Accepted |
| [0013](0013-settings-registry.md) | Settings as typed definitions, collected into one registry at build time | Accepted |
| [0014](0014-open-indexes-pool.md) | Open indexes charged to a pool of their own, least recently used closed first | Accepted |
| [0015](0015-cluster-crate-and-node-leases.md) | A cluster crate beside the metadata crate, and node leases on registers | Accepted |
| [0016](0016-native-transport-and-early-revoke.md) | OpenSearch's native transport, ported, and early revocation of a refused peer | Accepted |
| [0017](0017-mappings-and-documents-as-the-java-reads-them.md) | Mappings and documents read as the Java reads them; what the writer cannot index stays with a Java writer | Accepted |
| [0018](0018-tail-overlay-in-the-reader.md) | The reader searches pushed, unpublished writes through one small in-memory segment beside the commit | Accepted |
| [0019](0019-document-indexing-crate-below-reader-and-writer.md) | Turning a document into Lucene fields moves to a crate below the reader and the writer | Accepted |
| [0020](0020-janitor-crate.md) | The janitor is a crate beside the reader and the writer | Accepted |
| [0021](0021-proactive-takeover-from-the-node-log.md) | Proactive takeover reads the dead node's log, not a registry of its shards | Accepted |
| [0022](0022-nested-bucket-aggregations.md) | Nested bucket aggregations: one engine tree, a total bucket bound, a product-aware reservation | Accepted |
| [0023](0023-routing-stored-in-engine-documents.md) | A routed document stores its routing as a `_routing` field in the segment | Accepted |
| [0024](0024-node-packs-in-the-writer.md) | Node packs in the writer: one PUT for the segments a node publishes together | Accepted |
| [0025](0025-node-pack-collection.md) | Node pack collection | Accepted |
| [0026](0026-bucket-manifests-in-the-writer.md) | Bucket manifests in the writer: the shell's register, written as JSON first | Accepted |
| [0027](0027-hibernating-idle-writers.md) | An idle writer hibernates: engine closed, head kept | Accepted |
| [0028](0028-orphan-shard-sweep.md) | Orphan-shard sweep: an opt-in, default-off backstop that lists one key per shard prefix | Accepted |
| [0029](0029-giving-back-a-hibernated-head.md) | A hibernated shard gives its head back after a longer sleep | Accepted |
| [0030](0030-writer-shard-cap.md) | A writer node holds at most a set number of shards and lets an idle one go to open the next | Accepted |
| [0031](0031-document-sets.md) | Templates and pipelines are document sets behind one marker register | Accepted |
| [0032](0032-commit-push-in-a-booted-node.md) | A booted node pushes commits, and its own reader follows its own writer | Accepted |
| [0033](0033-retry-after-on-a-refused-write.md) | A refused single-document write carries a Retry-After read from the write limiter | Accepted |
| [0034](0034-writer-hand-off.md) | A writer node nearly at its cap, or far above the fleet's mean, hands idle shards to a member with room | Accepted |
| [0035](0035-the-tail-overlay-is-on-by-default.md) | The tail overlay is on by default | Accepted |
| [0036](0036-headroom-and-pinned-indexes.md) | A writer node can clear room in bursts and can pin indexes it never lets go of | Accepted |
| [0037](0037-per-index-store-usage.md) | The store counts its requests per index in a ledger beside the meter | Accepted |
| [0038](0038-snapshots-read-first.md) | Snapshots arrive read-only first, over the shell's records, behind a switch | Accepted |
| [0039](0039-knn-vectors-in-the-segment.md) | A `knn_vector` is written into the segment as the stock k-NN plugin writes it | Accepted |
| [0040](0040-repository-registration.md) | Repositories are registered and removed over REST, the Java's way, before snapshots are taken | Accepted |
| [0041](0041-tenant-cost-levels.md) | A tenant's cost is held to a budget by levels, in observe mode unless set to enforce | Accepted |
| [0042](0042-snapshots-taken-shallow-first.md) | Snapshots are taken shallow first, in the Java's protocol, behind the switch | Accepted |
| [0043](0043-security-configuration-as-document-sets.md) | Security configuration is five document sets, enforced at the edge from a node cache | Accepted |
| [0044](0044-ingest-pipelines-in-the-write-path.md) | Ingest pipelines run in the write path through a per-request hook, behind a switch | Accepted |
| [0045](0045-shallow-snapshot-restore.md) | A shallow snapshot is restored by copying its files into a new index, in the Java's order | Accepted |
| [0046](0046-plugin-requests-are-proxied-to-java-nodes.md) | Requests for plugins that are not ported are proxied to Java nodes, behind a setting | Accepted |
| [0047](0047-search-slots.md) | Searches are admitted in order by a slot gate that shapes who waits, behind a setting | Accepted |
| [0048](0048-retiring-a-node.md) | A node told to stop publishes its shards and gives their heads back | Accepted |
| [0050](0050-writes-to-one-shard-share-a-commit-window.md) | Writes to one shard share a commit window: the slot lock is released before the wait | Accepted |
| [0051](0051-authorization-at-the-route-by-action-table.md) | Authorization is decided at the route, from an action table, by the names in the path | Accepted |
| [0052](0052-extension-modules-on-a-wasm-abi.md) | Extension modules are sandboxed WASM on a bytes-in, bytes-out ABI, content-addressed in the store; no point is wired yet | Accepted |
| [0053](0053-the-analyzer-extension-point.md) | The analyzer point: an index pins a WASM module as a named analyzer at creation; every node runs it or refuses | Accepted |
| [0054](0054-parsed-mappings-are-shared-by-content.md) | Parsed mappings are shared across shards by content, in memory; the descriptor keeps its mapping inline | Accepted |
| [0055](0055-proactive-takeover-reapers-are-nodes-of-this-build.md) | Proactive takeover picks its reapers only among live nodes of this build | Accepted |
| [0056](0056-over-share-searches-wait-off-the-workers-in-a-bounded-queue.md) | A tenant's searches over its share of the workers wait off the workers in a bounded queue, and are refused with 429 past its size | Accepted |
| [0057](0057-a-rust-owner-answers-the-java-search-shard-action.md) | A Rust owner answers the Java search-shard action for a shard it writes, over a decoded subset of the request | Accepted |
| [0058](0058-modern-protocols-front.md) | HTTP/2, HTTP/3 and gRPC are served by an in-process front that bridges to the HTTP/1.1 server | Accepted |
| [0059](0059-the-writer-merges-a-shards-small-segments.md) | The writer merges a shard's small segments, behind a setting | Accepted |
| [0060](0060-stock-snapshot-restore-reads-a-source-repository.md) | A stock snapshot is restored by reading its repository through a source store | Accepted |
| [0061](0061-a-request-reports-its-own-store-requests.md) | A request that asks reports the store requests it made, in a response header | Accepted |
| [0062](0062-a-snapshot-is-exported-to-a-stock-repository-by-copying.md) | A snapshot is exported to a stock repository by copying the shard files | Accepted |
| [0063](0063-a-restore-may-run-in-the-background.md) | A restore may run in the background, behind a setting | Accepted |
| [0064](0064-commits-are-staged-and-checkpointed.md) | A routine publish stages its commit, and the register is written as a checkpoint | Accepted |
| [0065](0065-a-snapshot-captures-the-aliases-of-its-indices.md) | A snapshot captures the aliases of its indices, and a restore may bring them back | Accepted |
| [0066](0066-a-partial-snapshot-records-the-shards-it-could-not-capture.md) | A partial snapshot records the shards it could not capture, and a restore may bring it back | Accepted |
| [0067](0067-a-snapshot-captures-the-global-state-it-is-asked-for.md) | A snapshot captures the global state it is asked for, and a restore may bring it back | Proposed |
