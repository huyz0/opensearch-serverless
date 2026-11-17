# Segment fixtures

What stock OpenSearch's Lucene segment holds for a fixture's documents, so the writer's tests can check that the Lucene documents it builds are the ones a Java writer builds (`crates/writer/tests/segment_fixtures.rs`, [spec](../../docs/specs/indexing/SPEC.md)).

Each file is one JSON fixture:

- `name`, `description`: for people.
- `index`: the index the recorder creates, starting with `compat-segment-`.
- `mappings`: the create-index mappings, if any; without one, every field is dynamic.
- `documents`: `id` and `source` of each document, indexed in order; `"delete": true` in place of `source` deletes the id.
- `recorded_from` and `segment`: written by `record.py`, never by hand. `segment` is what `SegmentDump.java` reads from the one segment: `max_doc`; `fields` (number, name, index options, doc values type, skip index, `omit_norms`, point dimensions, payloads, term vectors); and `docs`, each with its stored fields in order (name, type, value), its `inverted` fields (norm, and each term with its frequency, positions and offsets as the field indexes them), its `doc_values` and its `points`. Bytes are a string when they are text, else `hex:` and their hex.

The test indexes each document's recorded `_source` bytes, which are what OpenSearch received.

## Recording

From a stock OpenSearch running on this machine, built and started as [fixtures/compat/README.md](../compat/README.md) describes, with `--data` its `path.data` and `--lucene` the `lucene-core` jar in its `lib/`:

```bash
python3 fixtures/segments/record.py --url http://localhost:9200 \
  --data <path.data> --lucene <distribution>/lib/lucene-core-10.5.1.jar fixtures/segments/*.json
```

For each fixture it creates the index (one shard, no replicas, no refresh), indexes the documents in one `_bulk` (one indexing thread, so one segment), flushes, copies the shard's files to a temporary directory, and runs `SegmentDump.java` on them (`java` 21, `lucene-core` only). Read the diff before committing.

## Fixtures

| File | Covers |
|---|---|
| [types.json](types.json) | every type the writer indexes with `index`, `doc_values`, `store`, `norms`, `index_options`, `position_increment_gap`, `ignore_above`, `null_value`, `ignore_malformed`; multi-fields, objects, arrays, coercion, dotted names, `_field_names`, `_ignored`, `@timestamp`'s skip index, non-ASCII offsets |
| [dynamic.json](dynamic.json) | dynamic fields of every JSON kind, the long-token split, an empty string, and a field first seen in a later document |
| [ids.json](ids.json) | `_id` in each of `Uid.encodeId`'s forms |
| [updates.json](updates.json) | updates and deletes: `__soft_deletes` on replaced and deleted documents, tombstones (`_tombstone`), a delete of a missing id; the test replays it through a shard ([spec](../../docs/specs/shard-engine/SPEC.md)) |

Recorded from stock OpenSearch 3.10.0-SNAPSHOT built from huyz0/OpenSearch `429b4cbc` (`./gradlew localDistro`), Lucene 10.5.1.
