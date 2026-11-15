# oss-format: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- The Java shell defines these formats. A Rust change that alters bytes on disk needs the same change in Java, and an ADR.
- Every decoder ships with a test over Java-written bytes, plus truncated and corrupt cases.
- Lengths and offsets read from a blob are checked before use (`clippy::arithmetic_side_effects`).
