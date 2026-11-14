# oss-blob: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- This is the only crate allowed an S3 client. Every attempt, retries included, calls `Meter::record`.
- A new request kind gets its price class in `src/op.rs`. A LIST call site carries `// LIST:`.
- Changes here are high-risk (S3 request pattern): ADR link and the needs-huy label.
