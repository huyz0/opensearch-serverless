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
