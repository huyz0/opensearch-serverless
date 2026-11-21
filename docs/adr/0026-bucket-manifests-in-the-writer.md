# 0026: Bucket manifest values in the writer: the shell's JSON form, a 1,024-member ceiling, compact refused

**Status:** Proposed

## Context

The shell groups small indices into buckets: one register `buckets/<id>/manifest` holds every member's latest commit ([bucket-manifest-reader](../specs/bucket-manifest-reader/SPEC.md)). Rust reads it and nothing writes it. The shell writes the compact `OSBM` form by default (`serverless.buckets.manifest_codec` is `auto`) and JSON when set to `json`; both are read by every node. Its limits are node settings: `serverless.buckets.max_members` (default 1,024, a hard ceiling), `target_bytes` (about 1 GB, where the stripe opens the next bucket) and `max_manifest_bytes` (2 MB, a stored-size cutoff for admitting members). This change adds only a value type with no request: `BucketManifest`, to build, edit one member, size and encode, and parse whole ([spec](../specs/bucket-manifest-writer/SPEC.md)).

## Decision

Only the three format choices this change makes:

- **JSON first.** `BucketManifest::encode` writes the JSON form, byte for byte the shell's `toBytes()`. The compact form is a later change (same format, with a codec).
- **A compact register is refused, not rewritten.** `BucketManifest::parse` returns `Unsupported` for `OSBM`, so a Rust writer cannot corrupt or silently downgrade a register Java wrote; it simply cannot take over one until the compact whole decode exists. The member reader reads either form.
- **A 1,024-member ceiling**, the shell's default `max_members`, hardcoded in Rust. A Java node configured above it can make a bucket Rust refuses (`TooLarge`); the Rust writer's own settings and the assignment piece must read the same configuration then, and until they do no Rust code writes a bucket.

The writer's later pieces (the member commit swap, assignment, the checkpoint path, the settings and their defaults) each get their own spec and, if they decide something, their own ADR. They keep what the shell has: a member's swap is judged against the member's own version, not the register's generation, and retries are bounded (the shell's `BucketCommits` retries 16 times); the switch is the existing `serverless.buckets.enabled` and the index and cluster bucket policies, default off, on only when measured to win.

## Consequences

- Requests: none; the type is not reachable from a node.
- Cost the later writer must weigh: in a mixed fleet a JSON rewrite of a bucket the Java wrote compact is larger on the wire, up to the 2 MB manifest cutoff and more in JSON, and every member's checkpoint rewrites the whole register. This is why the compact encoder is the next piece, and why bucketed placement stays behind a switch until measured.
- Memory: one manifest per in-flight edit, at most 1,024 commits.
- No LIST, no lock or lease: the fencing is the register's compare-and-swap, unchanged.
