# oss-types: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Identifiers are distinct newtypes; never pass a raw string or integer where an id is meant.
- No I/O and no dependency on another workspace crate.
