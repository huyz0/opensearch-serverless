# oss-mem: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- A reservation is taken before memory is used and released on drop; nothing allocates first and checks later.
- Exhaustion is a typed error the caller turns into 429 or backpressure, never a panic or an abort.
