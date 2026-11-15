# oss-reader: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Never writes to the object store. Never depends on `oss-writer`.
- Learns of commits by push from the writer; never polls per index.
- A search body is never parsed whole: it streams through `dsl::Recorder`, whose growth is reserved before each chunk. Every ported rule cites its Java at 3.10.0 in `port-map.tsv`.
