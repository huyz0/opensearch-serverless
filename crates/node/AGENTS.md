# oss-node: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- `src/main.rs` is wiring only; logic goes in `src/lib.rs` where tests reach it.
- Each role stays behind its Cargo feature.
