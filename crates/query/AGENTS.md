# oss-query: agent notes

Read [README.md](README.md) first, the root [AGENTS.md](../../AGENTS.md) for the invariants, and [ADR 0082](../../docs/adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md).

Local rules:

- No I/O, no threads, no async runtime. Data comes in through the caller's arguments; the crate never opens a socket or a file.
- Every parser bounds bytes, tokens, nesting depth and identifier length before it allocates in proportion to them, and never panics on any input.
- A construct the planner cannot push into a search body is a refusal (`Error::unsupported`), never a fallback that reads documents and filters them here.
- No generated search body holds a `script` key until the script-pushdown slice.
- Expected bodies and results in tests are marked `recorded` or `provisional`; a hand-written one is provisional.
