# oss-janitor: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- The janitor deletes. A blob is deleted only when it is in a term older than the published manifest's, nothing names it, and two consecutive sweeps saw it so. Weaken none of the three.
- The crate ships no "no pins" answer: a pin source that cannot tell says `Everything` and nothing is deleted.
- Every pass starts as a dry run (`serverless.janitor.dry_run`); never default it off without the fault suites' evidence.
- Nothing lists on a request path; a LIST here carries `// LIST:` and scales with a shard's blobs, never with time.
- Never depend on `oss-reader` or `oss-writer`.
