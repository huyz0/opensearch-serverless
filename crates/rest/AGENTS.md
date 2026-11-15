# oss-rest: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Bodies stream: never read a whole request or response into memory (ADR 0004).
- A body that would exceed its reservation is refused with 413 or 429, never buffered.
- Unknown parameters fail explicitly, as OpenSearch does.
