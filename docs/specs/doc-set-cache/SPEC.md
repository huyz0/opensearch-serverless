# Document set cache

## Serves

The node cache that [document sets](../doc-sets/SPEC.md) and [ADR 0031](../../adr/0031-document-sets.md) name as the precondition for reading every template at index creation. Invariants served: I-1 (every call metered), requests scale with nodes and bytes (a read of the whole set is one GET while it has not moved, not one a template), and bounded memory (a budget on what is kept).

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `template/TemplateCache.java` and `TemplateStore.all`.

- `DocSet::snapshot(cache, marker_buf, body_buf)` returns every document as an immutable `DocSnapshot` (version and documents in name order). One GET of the marker; if the cache's copy has the same version and the same names and content keys, that copy; otherwise the copy's documents whose name and content key are unchanged are reused (the same bytes, not copied) and the rest are read, one GET each.
- `DocSetCache` holds one snapshot, replaced whole, behind a lock that survives a panic. A snapshot retaining more than the budget (default 8 MiB, the Java's, counted as it counts: 128 bytes, the name, the key and the bytes of each document) is returned and not kept.
- A document dropped by a change mid-read restarts from the marker once; a second miss is `MissingContent`.
- Currency compares the names and keys as well as the version: a set recreated at a version the cache has seen is not taken for the held one, which the Java's version-only check would.

### Not in this change

Selecting templates by pattern and composing them, a setting for the budget, and the callers. Nothing calls `snapshot` yet. A cache of one document by key for `get` (the Java caches revisions too): `get` is 2 GETs and stays so.

## Acceptance criteria

- AC-1: A first snapshot of n documents is 1 + n GETs; an unchanged set is 1 GET and the same snapshot; one replaced document is 2 GETs and shares every other document's bytes; a deleted one is 1 GET; an empty set is 1 GET and is kept. Tests: `a_snapshot_reads_the_marker_and_only_the_documents_that_moved`, `an_empty_set_is_one_request_and_then_cached`, `a_changed_set_costs_what_changed_not_what_is_held`.
- AC-2: A set whose copy has other bytes under the same names, or other keys at a version already seen, is rebuilt, not served stale; a new version with the same keys costs the marker alone. Tests: `a_replaced_document_is_not_taken_for_the_cached_one`, `a_set_recreated_at_the_same_version_is_not_taken_for_the_cached_one`.
- AC-3: A snapshot over the budget is returned and read through every time; one at the budget is kept. Tests: `a_set_over_the_budget_is_returned_but_not_kept`, `a_set_just_under_the_budget_is_kept`.
- AC-4: A document dropped mid-read restarts from the marker; one the store lost is `MissingContent`. Tests: `a_document_dropped_mid_read_restarts_from_the_marker`, `a_document_the_store_lost_is_reported`.

## Test plan

`crates/meta/tests/descriptors/doc_cache.rs` over the emulator, S3 on loopback and the filesystem store, with exact request counts.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `snapshot`, set unchanged | 0 | 1 | 0 | 0 | 1 |
| `snapshot`, k documents moved | 0 | 1 + k | 0 | 0 | 1 + k (one buffer, one after another) |
| `snapshot`, cold, n documents | 0 | 1 + n | 0 | 0 | 1 + n |

## Switch

None needed: nothing calls it yet. The wiring gets the setting.

## Risks

- A snapshot reads moved documents one after another; a cold node reads up to 1,001 once, then 1 each create. Index creation does not ship before the cache is warm in practice, and the cold read is bounded by the cap.
- The cache keeps up to 8 MiB a set, four sets a node: 32 MiB worst case, charged to no pool yet. The wiring charges it.
