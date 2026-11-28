# oss-ml: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- This crate is the only place `lucene-ml` is named; `oss-node` reaches it through these wrappers.
- No I/O of its own: bytes in, bytes out. The registry in `oss-node` owns every blob and every request.
- A credential value never appears in an error text or a log line.
