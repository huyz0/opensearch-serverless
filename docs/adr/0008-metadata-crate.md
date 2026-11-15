# 0008: A metadata crate between the stores and the roles

**Status:** Proposed, 2026-11-15.

## Context

The Java shell keeps every piece of cluster metadata in registers: index descriptors, aliases, tombstones, change stripes, cluster config, templates. The writer needs it to create and delete indexes, the coordinator to resolve names, and later the reader to follow aliases. It is more than a record format (`oss-format` stays pure parsing and writing over bytes) and less than a role, and putting it in the writer would make the coordinator and the reader depend on the writer, which the layering forbids.

## Decision

- A new crate, `oss-meta`, at layer 2: it depends on `oss-blob` (registers) and `oss-format` (the records' bytes), and every role may depend on it. The roles move up one layer: reader and writer 3, coordinator 4, node 5, compat 6.
- It ports the Java's `metadata` package piece by piece, starting with `Names` and `DescriptorStore`. Its records' bytes are `oss-format`'s, written as the Java's Jackson writes them, so either runtime can swap a register the other wrote.
- Every change is a register compare-and-swap, a conditional create or a conditional delete. Name uniqueness is arbitrated by the store's conditional create, as in the Java; there is no lock, lease or leader.
- Calls take the caller's buffer, which bounds each record read or written: nothing allocates per record but the key.
- How a delete keeps a recreated name from meeting an old generation (a conditional delete, or a tombstone swapped in after a marker) is the caller's choice, as the Java's `setConditionalDelete`; the Java's probe that decides it is ported later.

## Consequences

- One more crate in the request path's allocation check (`scripts/check-alloc.py`).
- Records' settings and unit layouts are kept as JSON bytes until the settings registry types them; a record built in Rust carries whatever settings object its caller gives.
- Listing names (`DescriptorListing`) will be the crate's only LIST, behind `// LIST:`, when it is ported.
