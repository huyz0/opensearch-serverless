# oss-cluster: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Every change to a lease or the members index is a register compare-and-swap, a conditional create or a conditional delete: never a lock or a leader.
- No LIST, anywhere: members are named by the members index, and a key is derived (`oss_format::lease_key`).
- Records are `oss-format`'s to read and write; this crate never builds JSON or digests by hand.
- A lease is the only place a successor finds a node's unpublished write-log epochs: nothing here removes one that names an epoch.
