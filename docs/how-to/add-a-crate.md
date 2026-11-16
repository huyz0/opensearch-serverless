# Add a crate

1. Decide its layer in [architecture.md](../architecture.md). If it fits inside an existing crate as one module, do that instead.
2. Create `crates/<name>/` with `Cargo.toml` (copy a sibling's: `name = "oss-<name>"`, workspace package fields, `[lints] workspace = true`), `src/lib.rs` with a crate doc comment, `README.md` with `## Owns`, `## Depends on`, `## Entry points`, and a short `AGENTS.md` with the crate's local rules.
3. Add it to `members` and `[workspace.dependencies]` in the root `Cargo.toml`.
4. Add it to `LAYERS` in `scripts/check-layering.py`.
5. Add a row to the crate table in [architecture.md](../architecture.md) and run `scripts/gen-crate-map.py` to regenerate the graph.
6. A new workspace crate is high-risk (`check-pr.py` flags the added `Cargo.toml`): ADR and `needs-huy`.
7. Add `crates/<name>/CLAUDE.md` containing only `@AGENTS.md`, so Claude Code reads the crate's rules.

**Checks:** `scripts/check-crate-layout.py` (including the `CLAUDE.md` adapter), `scripts/check-layering.py`, `missing_docs`.
