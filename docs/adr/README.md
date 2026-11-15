# Architecture decision records

One file per decision: `NNNN-short-title.md`, numbered in order, never renumbered or deleted. A decision that is overturned stays, with its status changed to `Superseded by NNNN`.

**When one is required:** a PR that touches blob formats, durability or fencing, the workspace's crates or lints, public API, the S3 request pattern, or a default links an ADR or adds one (`scripts/check-pr.py`). A third-party dependency change alone does not need one. A new ADR starts as **Proposed**; only the owner moves it to Accepted.

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
| [0010](0010-one-home-per-rule.md) | One home per rule, and a harness that checks itself | Proposed |
