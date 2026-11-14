# Architecture decision records

One file per decision: `NNNN-short-title.md`, numbered in order, never renumbered or deleted. A decision that is overturned stays, with its status changed to `Superseded by NNNN`.

**When one is required:** a PR that touches blob formats, durability or fencing, crate dependencies, public API, the S3 request pattern, or a default links an ADR or adds one (`scripts/check-pr.py`). A new ADR starts as **Proposed**; only the owner moves it to Accepted.

Each record has: **Status** (Proposed, Accepted, Superseded), **Context** (the forces, with numbers where they exist), **Decision**, **Consequences** (what gets easier, what gets harder, what it rules out).

| ADR | Title | Status |
|---|---|---|
| [0001](0001-strangler-and-crate-layout.md) | Strangler beside the Java shell, layered crates | Accepted |
| [0002](0002-runtime-dependencies.md) | How hsearch, lucene-rust and pstore are consumed | Proposed |
