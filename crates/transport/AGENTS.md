# oss-transport: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- OpenSearch and the Java shell define this wire. Every encoding is tested against bytes the Java's own classes wrote (`fixtures/transport/`); a change that alters bytes needs a new recording, never an edited expectation.
- Lengths and counts read off the wire are checked before use, and a length is never trusted to size an allocation beyond the bytes actually present or the frame bound.
- This crate makes no S3 request and depends on no role crate.
