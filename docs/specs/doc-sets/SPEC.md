# Document sets

## Serves

The plan's index-templates and ingest-pipelines parts of the snapshots, templates, ingest and auth milestone: the store those REST handlers will read and write. Stock composable and component templates are how a schema is authored (the schema-sharing decision of 2026-11-12: no new schema API); a document set holds them, and ingest and search pipelines, in the object store with no cluster state.

Invariants served:
- **I-1:** every store call is metered.
- **Fencing:** a set changes only by one compare-and-swap of its marker, never a lock.
- **Requests scale with nodes and bytes, no LIST:** the marker names every document, so reading one is 2 requests and reading the set is 1 plus one a document, bounded by the cap.
- **Bounded memory:** a document and a marker each fit a caller's buffer.

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `template/TemplateStore.java` (the indexed form) and `metadata/RegisterMap.java` (`indexTemplates`, `componentTemplates`, `pipelines`, `searchPipelines`). A decision record is not needed: the design is the Java's, and it adds no request class.

### In `oss-format`

- `DocKind` and `doc_set_marker_key` / `doc_content_key`: `templates/index/`, `templates/component/`, `pipelines/`, `search_pipelines/`, each holding the marker register `_version` and content blobs `blob-<content>`.
- `DocSetMarker`: `{"version":n,"entries":{name: content}}`, entries in `String` order, as `writeMarker`. `parse` reads as `readMarker` (a missing version is 0, no entries is none, unknown fields skipped) and is stricter where the Java coerces.

### In `oss-meta`

- `DocSet::{version, names, get, for_each, put, delete}`. A put writes the bytes under a fresh key, then swaps the marker (or creates it); the replaced bytes are deleted afterwards unless another name reads them. A lost swap reads again, up to 10 attempts, writing the bytes once.
- Content keys written here are `blob-<22-character id>`; the Java names its own `blob-<sha256>-<id>` and never parses one, so either reads the other's.

### Not in this change

The form a store had before its marker held entries (a bare counter), which a Java node carries over on its first change: Rust refuses it as `Unsupported`. No node-local cache of the set (the Java's `TemplateCache`), which comes with index creation reading every template. The REST handlers, template matching and composition, stored scripts, and a cluster setting to switch the feature, all of which come with the coordinator wiring: nothing calls `DocSet` yet.

## Acceptance criteria

- AC-1: A marker as the Java's `writeMarker` writes it reads and writes back the same bytes. Entries are written in `String` (UTF-16) order. A missing version or entries reads as the Java reads it. The four sets' keys are the Java's. Tests: `the_javas_marker_reads_and_writes_the_same_bytes`, `entries_are_written_in_the_javas_tree_map_order`, `an_empty_marker_has_no_entries`, `a_missing_version_or_entries_reads_as_the_java_reads_it`, `keys_are_the_javas_register_map_paths`.
- AC-2: Bounds. More than 1000 entries are refused to read and to write; 1000 entries of the longest name and content key fit `MAX_DOC_SET_MARKER_BYTES`; a short buffer is refused; a counter from before the indexed form is `Unsupported`; a marker the Java could not write (a version or entry of the wrong type, a content key that is not a path segment, a repeated field, trailing bytes) is refused. Tests: `more_entries_than_the_cap_are_refused_in_both_directions`, `the_cap_of_entries_at_their_longest_fits_the_bound`, `a_short_buffer_is_refused`, `a_counter_from_before_the_register_held_entries_is_unsupported`, `a_marker_the_java_could_not_have_written_is_refused`, `a_key_is_refused_for_a_bad_base_or_content`.
- AC-3: On each backend a document is put, read, replaced and deleted, each change moving the version by 1; a marker and content written as the Java writes them read as a set, and deleting one of two names sharing a blob leaves it for the other; `require_absent` refuses an existing name and keeps its document; a full set refuses a new name and still replaces an existing one. Tests: `a_document_is_put_read_replaced_and_deleted`, `the_javas_marker_and_content_read_as_a_set`, `require_absent_refuses_an_existing_name_and_keeps_the_document`, `a_full_set_refuses_a_new_name_and_still_replaces`.
- AC-4: Requests, counted on each backend (a swap also costs 1 ranged GET on the filesystem store): the first put is 1 GET and 2 PUTs; a replace is 1 GET, 2 PUTs and 1 DELETE; a delete is 1 GET, 1 PUT and 1 DELETE, and of an absent name 1 GET; a read of one is 2 GETs; the names are 1 GET; every document of a set of one is 2 GETs. A bad name or an oversize document sends nothing. Tests: `request_counts_per_operation`, `a_bad_name_or_document_sends_nothing`.
- AC-5: Contention. A put that loses 3 swaps reads the marker 4 times and writes its bytes once; one that loses all 10 attempts is `Contended`, leaves no bytes behind and changes nothing; a delete that loses all is `Contended`; a put that finds its name taken on retry under `require_absent` drops its bytes. Tests: `a_put_retries_a_lost_swap_and_writes_its_bytes_once`, `a_put_that_loses_every_attempt_leaves_no_bytes_behind`, `a_put_that_finds_the_name_taken_on_its_retry_drops_its_bytes`.
- AC-6: A document the marker names that the store does not hold is `MissingContent`, after one more read of the marker. Test: `a_document_the_marker_names_but_the_store_lost_is_reported`.

## Test plan

- AC-1 and AC-2: `crates/format/tests/doc_sets.rs`. ⚠️ The Java marker there is the bytes `TemplateStore.writeMarker` produces by reading its source (compact `XContentBuilder` output, `TreeMap` order); it was not recorded from a running Java node, so a fixture from one is the open follow-up.
- AC-3 to AC-6: `crates/meta/tests/descriptors/doc_sets.rs` over the emulator, S3 on loopback and the filesystem store. Conflicts are planted by the suite's `Quirky` store and a hook that plays the other writer.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `version`, `names` | 0 | 1 | 0 | 0 | 1 |
| `get` | 0 | 2 | 0 | 0 | 2 |
| `for_each`, n documents | 0 | 1 + n | 0 | 0 | 1 + n (one buffer, one after another) |
| `put`, first | 2 | 1 | 0 | 0 | 3 |
| `put`, replace | 2 | 1 (and 1 ranged on fs) | 0 | 1 | 3 |
| `delete` | 1 | 1 (and 1 ranged on fs) | 0 | 1 | 2 |
| each lost attempt of a put or delete | +1 swap answered 412 | +1 | 0 | 0 | +1 or +2 |

Paid at the rate operators change templates; nothing reads a set on a timer.

## Switch

None needed: nothing calls `DocSet` yet. The setting that governs reading templates at index creation arrives with that wiring, with the cache.

## Risks

- A put whose bytes landed but whose marker swap then failed with a store error drops the bytes best-effort; if that fails too, the blob stays, and nothing sweeps it (no LIST). Likewise the replaced bytes after a good swap. The cost is storage, never a wrong read.
- `for_each` reads the documents one after another, and may visit a name twice if a change lands mid-walk; a set of 1,000 is 1,001 requests until the node cache lands. It is called only from index creation, which will not ship before the cache.
- Rust never runs the legacy-form migration, so a bucket whose store a Java node never carried over must be touched by Java first. The refusal names it.
