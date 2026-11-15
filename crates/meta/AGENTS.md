# oss-meta: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Every change to a name is a register compare-and-swap, a conditional create or a conditional delete: never a lock, a lease or a leader. Name uniqueness is the store's to arbitrate.
- No LIST on any path here except an explicit listing, and every one carries `// LIST:`. A name's key is derived (`oss_format::descriptor_key`).
- Records are `oss-format`'s to read and write; this crate never builds JSON by hand.
- Validate a name before it becomes a key: a name is a path segment on the filesystem store.
