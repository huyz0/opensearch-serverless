# Add an extension point

Extensions are C modules and WASM modules on an ABI this repository defines, informed by hsearch's extension ABI v1 but not depending on it ([ADR 0002](../adr/0002-runtime-dependencies.md)). Points planned by the assessment: analyzer, query, ingest, scoring, aggregation, filter, rest, job, auth.

Rules for a new point:

- A module receives bytes and returns bytes. It never gets a handle to an index, blob or socket.
- One call carries a block (1,024 docs, 16 documents, one batch), never one value per call.
- Points that build terms (analyzer, ingest) are deterministic and import nothing, so recovery reproduces them.
- An index pins `{module, point, version, sha256}`; a node without the pinned module refuses the index.
- Host calls a module makes are charged to the tenant's request counters, and module limits (fuel, memory, deadline) answer 429 when exceeded, never a silent fallback.

A worked example is the `analyzer` point ([ADR 0053](../adr/0053-the-analyzer-extension-point.md), [spec](../specs/extension-analyzer/SPEC.md)). Its order of work, which a new point follows:

1. The point's codec and a module run as that point (`oss-ext`, `oss-ext-wasm`), with the host refusing malformed output.
2. The index pins the module at create (a setting the coordinator validates, resolves limits for and records), gated by a cluster setting that stays off.
3. Every site that would build the thing a built-in builds takes the module instead or refuses: the writer, replay and takeover, the reader's open and its queries, the overlay, highlighting. A node that cannot run the pin refuses and never falls back.
4. A failed call answers by what a caller can do (429, 502, 400), not a generic 500.
5. The soak, replay equality and measurements are recorded before the setting is turned on anywhere.

**Checks:** `scripts/check-extension-pins.py` (no shipped configuration turns analyzer pins on) and `scripts/check-analyzer-sites.py` (a new file that builds a built-in analyzer is reviewed against pins).
