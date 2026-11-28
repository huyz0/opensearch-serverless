# oss-anomaly: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- Pure compute: no I/O, no clock, no thread, no dependency. Memory is fixed at construction (`Params::reserved_bytes` is the sum of every array's capacity) and scoring or updating a point allocates nothing; `tests/alloc.rs` holds that with the counting allocator.
- Everything is seeded: the same seed and input give the same output and the same checkpoint bytes. Never read the clock or an OS random source here.
- A checkpoint is untrusted input when restored: every length comes from the validated `Params`, every index is checked, and the trees are verified structurally before a model is returned.
- The thresholder's constants live in `threshold.rs` and nowhere else; a change to one is checked against the fixtures, never tuned to pass.
