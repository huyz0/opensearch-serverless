# 0001: Strangler beside the Java shell, layered crates

**Status:** Accepted, 2026-11-14.

## Context

The Java serverless shell works and is under a code freeze. A Rust rewrite promises lower memory per held shard, no GC pauses and one cache instead of two, but the OpenSearch layer above Lucene (DSL, mappings, reduce, REST compatibility) is large. A big-bang port would deliver nothing for months and let the blob formats drift.

## Decision

- Build the Rust service as a strangler: reader role first, then writer, then coordinator. Each role reads and writes the Java shell's blob formats, so both run against the same bucket in a mixed fleet.
- One crate per role, over a shared `oss-blob` (the only S3 path) and `oss-types`, with dependencies pointing downward only and the two middle-layer roles independent.
- Each role is a Cargo feature of the one node binary.

## Consequences

- Every stage ships something usable and can stop at its gate.
- The blob formats are a contract both codebases obey; a format change lands in Java and Rust together.
- A mixed fleet doubles the operational surface during the migration.
