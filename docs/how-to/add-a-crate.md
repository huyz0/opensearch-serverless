# Add a crate

1. Decide its layer in [architecture.md](../architecture.md). If it fits inside an existing crate as one module, do that instead.
2. Create `crates/<name>/` with `Cargo.toml` (copy a sibling's: `name = "oss-<name>"`, workspace package fields, `[lints] workspace = true`), `src/lib.rs` with a crate doc comment, `README.md` with `## Owns`, `## Depends on`, `## Entry points`, and a short `AGENTS.md` with the crate's local rules.
3. Add it to `members` and `[workspace.dependencies]` in the root `Cargo.toml`.
4. Add it to `LAYERS` in `scripts/check-layering.py`.
5. Add a row to the crate table in [architecture.md](../architecture.md) and run `scripts/gen-crate-map.py` to regenerate the graph.
6. A new crate changes crate dependencies, so the PR is high-risk: ADR and `needs-huy`.

**Checks:** `scripts/check-crate-layout.py`, `scripts/check-layering.py`, `missing_docs`.
