# oss-blob: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- This is the only crate allowed an S3 client. Every attempt, retries included, calls `Meter::record`.
- A new request kind gets its price class in `src/op.rs`. A LIST call site carries `// LIST:`.
- The S3 client records an attempt inside `S3Store::send`, just before the request goes out, and `S3Store::run` counts each failed attempt once. A new request goes through both.
- `tests/sigv4/` is AWS's published test suite, kept unchanged as test data; nothing there is ever sent.
