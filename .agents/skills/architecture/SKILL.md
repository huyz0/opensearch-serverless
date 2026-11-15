---
name: architecture
description: "WHAT: Crate layout, layers and where code belongs. USE WHEN: adding or moving a crate, module or file, changing Cargo dependencies, or deciding which crate owns new code."
---

# Architecture

The layers and the crate graph: [docs/architecture.md](../../../docs/architecture.md) (generated from `cargo metadata`). The code-shape rules, once: [principles.md, Code shape](../../../docs/principles.md#code-shape).

## Procedure

- **Placing code:** pick the lowest crate that owns the concern. Code both reader and writer need moves down into `oss-blob`, `oss-types` or a new lower crate, never sideways.
- **`lib.rs`** holds the crate doc, the module list and re-exports, not logic. **`main.rs`** is wiring only; logic lives in the library, where tests reach it.
- **New crate:** follow [docs/how-to/add-a-crate.md](../../../docs/how-to/add-a-crate.md), including its `LAYERS` entry.
- **New dependency:** [ADR 0002](../../../docs/adr/0002-runtime-dependencies.md) says which are allowed. One that is allowed must also be pinned (exact lock or git rev), maintained, minimal (diff `Cargo.lock` for what it pulls in) and Apache-2.0 compatible; say so in the PR.
- **A decision with trade-offs** gets an ADR in `docs/adr/`.

## Enforced by

- `scripts/check-layering.py` (layers, siblings, unplaced crates); `scripts/check-crate-layout.py` (README sections, `AGENTS.md` and its `CLAUDE.md` adapter, `src/lib.rs`, workspace lints).
- `scripts/check-module-size.py`; `scripts/check-s3-access.py`; `missing_docs = "deny"` and `unsafe_code = "deny"` (local allows per [unsafe.md](../../../docs/unsafe.md)) in the workspace lints.
- Not mechanical: one concern per module, no dumping-ground modules, and the dependency review. Review.
