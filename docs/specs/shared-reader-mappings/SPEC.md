# Shared reader mappings: opened indexes with the same mapping text hold one parsed copy

## Serves

The shared-schemas goal (about 100 distinct schemas across 10M tenants), under [ADR 0054](../../adr/0054-parsed-mappings-are-shared-by-content.md), reader slice (the writer slice is [shared-mappings](../shared-mappings/SPEC.md)). A reader holding thousands of open shards of a few mappings keeps one parsed mapping and one JSON source a distinct text, not one of each a shard.

## Change

- `oss_doc::mapping::TextInterner<T>`: the weak, lock-sharded table of `MappingInterner`, generic over the value; `MappingInterner` is a wrapper of it. No behaviour change to the writer.
- The reader's `IndexMeta.mapping` is an `Arc<SharedMapping>` (the parsed fields and the JSON source, `Deref` to the fields, `json()` for the source) taken from a process-wide `TextInterner` keyed by the descriptor's escaped mapping text. A hit skips the unescape and the parse.
- `IndexMeta.mapping_json` is removed; callers read `meta.mapping.json()`.
- No request, format or setting changes.

## Acceptance criteria

- AC-1: two `meta_of` calls on descriptors with equal mapping text return metas whose `mapping` is the same `Arc` (`Arc::ptr_eq`); different text returns different ones; a text that does not parse is an error and is not stored.
- AC-2: the JSON source and the fields answer as before: `mapping.get`, `len` and `json()` of a shared meta equal those of an unshared parse of the same text.
- AC-3: after every meta of a mapping is dropped its entry is gone from the table (no leak of a closed index's mapping).
- AC-4: no blob request is added: the table is in memory and keyed by text the open already read.

## Tests

`crates/reader/src/shard_files.rs` unit tests (AC-1 to AC-3); AC-4 holds by construction (the interner holds no store).

## S3 request budget

Zero added; no LIST.

## Switch

None, per the ADR: no request, format or answer changes.

## Risks

- Identical meaning in different text is not shared (as in the writer slice).
- The table is process-wide: tests that count its live entries use texts no other test uses.
