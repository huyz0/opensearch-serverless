---
name: architecture
description: "WHAT: Crate layout, layers and where code belongs. USE WHEN: adding or moving a crate, module or file, changing Cargo dependencies, or deciding which crate owns new code."
---

# Architecture

Layers, lowest first: `oss-types` (0) ← `oss-blob` (1) ← `oss-reader`, `oss-writer` (2, siblings) ← `oss-coordinator` (3) ← `oss-node` (4). A crate depends only on lower layers. Map and flows: [docs/architecture.md](../../../docs/architecture.md).

## Rules

- **Downward only; siblings independent.** Code both reader and writer need moves down into `oss-blob` or `oss-types`, or into a new lower crate.
- **S3 only through `oss-blob`.** No other crate names an S3 client.
- **One concern per module, under 500 lines** (1,000 hard). No `util`, `misc` or `common` modules: name the concern.
- **`lib.rs` holds the crate doc, the module list and re-exports**, not logic.
- **`main.rs` is wiring only;** logic lives in the library where tests reach it.
- **Private by default;** `pub` only for what another crate uses, and documented.
- **New crate:** follow [docs/how-to/add-a-crate.md](../../../docs/how-to/add-a-crate.md), including its `LAYERS` entry.
- **A decision with trade-offs gets an ADR** in `docs/adr/`.

## Enforced by

- `scripts/check-layering.py` (layers, siblings, unplaced crates).
- `scripts/check-crate-layout.py` (README sections, `src/lib.rs`, workspace lints).
- `scripts/check-module-size.py`; `scripts/check-s3-access.py`.
- `missing_docs = "deny"` and `unsafe_code = "deny"` (local allows per [unsafe.md](../../../docs/unsafe.md)) in the workspace lints.
- Not mechanical: "one concern per module" and "no dumping-ground modules". Review.
