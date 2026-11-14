# Add an extension point

Extensions use hsearch's extension ABI v1 (C modules and WASM modules) once it is wired in ([ADR 0002](../adr/0002-runtime-dependencies.md)). Points planned by the assessment: analyzer, query, ingest, scoring, aggregation, filter, rest, job, auth.

Rules for a new point:

- A module receives bytes and returns bytes. It never gets a handle to an index, blob or socket.
- One call carries a block (1,024 docs, 16 documents, one batch), never one value per call.
- Points that build terms (analyzer, ingest) are deterministic and import nothing, so recovery reproduces them.
- An index pins `{module, point, version, sha256}`; a node without the pinned module refuses the index.
- Host calls a module makes are charged to the tenant's request counters, and module limits (fuel, memory, deadline) answer 429 when exceeded, never a silent fallback.

**Checks:** none yet; the ABI crate brings its own.
