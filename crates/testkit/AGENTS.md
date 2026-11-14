# oss-testkit: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Dev-dependency only: no crate names it under `[dependencies]`.
- Helpers measure and assert; they never hide a failure or loosen a bound.
