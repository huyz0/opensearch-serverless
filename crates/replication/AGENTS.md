# oss-replication: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- The leader's store is read-only by construction: `RemoteStore` exposes no method that writes, deletes or lists. Adding one breaks the compile-fail test; do not add one.
- A remote is readable only under an operator-listed root (`OSS_REPLICATION_REMOTE_ROOTS`). The check is `oss_blob::SourceRoots`, shared with the stock restore; never a second copy.
- Never depend on `oss-reader`, `oss-writer` or `oss-janitor`.
- Nothing here lists, and nothing polls without an explicit start.
