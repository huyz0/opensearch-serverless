# Shared mappings: shards with the same mapping text hold one parsed copy

## Serves

The shared-schemas goal (about 100 distinct schemas across 10M tenants), under [ADR 0054](../../adr/0054-parsed-mappings-are-shared-by-content.md) (accepted after independent review). First slice: the writer's shard. A node holding thousands of open shards of a few mappings keeps one parsed `Mapping` a distinct text, not one a shard.

## Change

- `oss_doc::mapping::MappingInterner`: `get(text) -> Result<Arc<Mapping>, MappingError>`, keyed by the parse options and the exact text; 16 lock shards by hash; weak entries; dead entries dropped on lookup and by a per-lock-shard sweep past twice the live count at the last sweep (at least 64).
- The writer's shard holds `Arc<Mapping>`: `Shard::new` takes `impl Into<Arc<Mapping>>`; `ShardSlot::open`, `open_shard_from` and `open_taken` carry an `Arc`; the public `open_shard` takes `impl Into<Arc<Mapping>>`. The head opener parses through the node's interner. A growing write wraps the grown mapping in an `Arc` the shard keeps as its own.
- No request, format or setting changes.

## Acceptance criteria

- AC-1: two `get` calls with the same text return the same `Arc` (`Arc::ptr_eq`); different text returns different ones; text that does not parse is an error and is not stored.
- AC-2: after every holder of a mapping is dropped the entry is gone: churning through 10,000 distinct mappings held one at a time leaves the table's entries bounded by the live holders plus the sweep threshold, not by the number seen.
- AC-3: opening N shards of one mapping text through the opener holds one parsed `Mapping`: the opener's table has one live entry for N shards. (The byte figure, 210 KB a 150-field parse, was measured once with a throwaway counting allocator; no test keeps it, see the ADR.)
- AC-4: a shard whose mapping grows holds its own new mapping and the others keep the shared one unchanged; the grown shard no longer holds the shared one.
- AC-5: many threads calling `get` with the same text get one `Arc`; many threads with distinct texts each get their own and none blocks on another's parse.
- AC-6: no blob request is added: the interner is a table in memory with no store, and parses descriptor text the open already read.
- AC-7: a hit costs less than a tenth of a parse of a 150-field mapping (a relative bound, not an absolute time).

## Tests

`crates/doc/tests/mapping_intern.rs` (AC-1, AC-2, AC-5, AC-7), `crates/node/tests/roles/writer_opener.rs` (AC-3), `crates/writer/tests/shard.rs` (AC-4); AC-6 holds by construction (`intern.rs` holds no store).

## S3 request budget

Zero added; no LIST. One hash of the mapping text a shard open.

## Switch

None, per the ADR: no request, format or answer changes. It becomes a setting if a measured shard-open or write p99 regression over 5% appears.

## Risks

- Identical meaning in different text is not shared. Templates and repeated creates produce identical text; the first measurement on real tenants decides whether a canonical form is worth building.
- A drifted index keeps its own copy, and the copy is large until the mapping itself is shrunk (the next task, [ADR 0054](../../adr/0054-parsed-mappings-are-shared-by-content.md) "Not decided here").
