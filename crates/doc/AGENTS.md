# oss-doc: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- A change here changes what both the writer and the reader's overlay index; the writer's conformance fixtures must still pass.
- Never depends on `oss-reader` or `oss-writer`. No object-store access.
- Every ported rule cites its Java at 3.10.0 in `port-map.tsv`.
