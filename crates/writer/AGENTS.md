# oss-writer: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- One write log per node, never per shard. One immutable blob per segment.
- Commit metadata rides in uploads already made; no extra PUTs.
