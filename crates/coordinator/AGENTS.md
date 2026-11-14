# oss-coordinator: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Parses, routes, fans out and reduces. Shard work belongs to the reader or writer.
- Unknown request parameters fail explicitly.
