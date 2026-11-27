# Indexing: a parsed document as the Lucene fields OpenSearch writes

## Serves

Parity plan R6.2 (roadmap stage 3, [roadmap](../../roadmap.md)): the writer turns a document read against its mapping ([mappings](../mappings/SPEC.md)) into the Lucene document OpenSearch indexes for it, field for field, so a segment the Rust writer builds reads the same as one a Java writer builds for the same requests. [ADR 0017](../../adr/0017-mappings-and-documents-as-the-java-reads-them.md).

## Change

- `oss-writer` gains `lucene` (`crates/writer/src/lucene/`), a port of what each field mapper's `parseCreateField` adds to `ParseContext.Document` and of the metadata mappers (server, OpenSearch 3.10 at `429b4cbc`):
  - `_id` (`Uid.encodeId`: numeric, URL-safe base64 and UTF-8 forms; indexed as one term, stored as bytes), `_seq_no` (a point and numeric doc values), `_primary_term` and `_version` (numeric doc values), `_source` (stored bytes), in the Java's order; `_ignored` (indexed and stored) after the document's fields.
  - `text`: the standard analyzer (`StandardTokenizer`, 255-char tokens, lowercased, no stop words), positions and offsets across values with the position increment gap (100, or the mapping's) and an offset gap of 1, `index_options`, BM25 norms (`SmallFloat.intToByte4` of the token count), `store`.
  - `keyword`: one term a value, sorted-set doc values, `norms`, `store` as bytes.
  - numbers: a point and sorted-numeric doc values (floats and doubles as sortable integers), `store` as the Java's `StoredField` type (`int` for byte, short and integer).
  - `date`: a long point and sorted-numeric doc values; `@timestamp` gets a doc-values skip index, as `isSkiplistDefaultEnabled` gives it; `store` as a long.
  - `boolean`: the term `T` or `F`, sorted-numeric doc values 1 or 0, `store` as the string.
  - `_field_names` for a field without doc values or norms, with every parent path, where the mapper adds it.
  - `index`, `doc_values` and `store` set to `false` leave out what the Java leaves out.
- `lucene::Indexer::document` builds the engine's `ExplicitDocument` from a `ParsedDocument`, its id, sequence number, primary term, version and source. Each field is registered with the `FieldInfo` its mapper gives it, through `lucene::FieldNumbers`, which the engine's `IndexWriter` implements: the engine numbers fields for every segment of the writer and refuses a schema change, as `FieldInfos.FieldNumbers` does, so numbers follow the order a Java `IndexingChain` first sees the fields and hold across segments and reopened writers. A field's spec is read from the mapping each document, so a mapping update reaches the engine, which refuses one its schema cannot take (`norms` turned off on an indexed field).
- keyword `index_options: freqs` indexes frequencies (`TextParams.toIndexOptions`); `_ignored` holds each malformed field once, in the order Java's `HashSet` iterates them.
- `document::parse` stops listing a keyword over `ignore_above` in `ignored`: the Java drops the value without adding the field to `_ignored` (the recorded segments show it).
- Recording: `fixtures/segments/record.py` indexes a fixture's documents into stock OpenSearch in one `_bulk`, flushes, and stores what `fixtures/segments/SegmentDump.java` (lucene-core only) reads from the segment: field infos, and for each document its stored fields, terms with frequencies, positions and offsets, norms, doc values and points.
- Not changed: a term over Lucene's 32,766-byte limit (refused by the engine, not yet with the Java's message), sequence numbers, versions, deletes and updates (the engine's soft deletes), index settings, the memory reservation (made with bulk, R6.3), the Java's message for a schema change within one in-memory segment (`Inconsistency of field data structures across documents ...`, which needs the segment's document count; the shard engine's), segment flush and upload (R6.3, R6.4), dynamic templates, `copy_to`, nested and the types outside the mappings subset (refused as unsupported there).

## Acceptance criteria

- AC-1: for every fixture of `fixtures/segments/`, the field infos the engine writes for the Rust documents equal the recorded ones: names, numbers, index options, doc values type, skip index, norms, point dimensions.
- AC-2: for every document of those fixtures, the stored fields (in order, with their types), each field's terms with frequencies, positions and offsets, norms, doc values and points equal the recorded ones.
- AC-3: every id form of `fixtures/segments/ids.json` (numeric, leading zero, base64 with and without the escape byte, UTF-8 for the rest) encodes to the recorded `_id` bytes.
- AC-4: the documents are accepted by the engine and committed as one segment of the recorded document count.
- AC-5: a field keeps its number in a later segment of the same writer and in a reopened writer, read back from the segments; a mapping update the engine's schema cannot take is refused as an `illegal_argument_exception`, and the field keeps its schema.

## Test plan

`crates/writer/tests/it/segment_fixtures.rs` builds each fixture's documents with `document::parse` and `lucene::Indexer`, writes them with the engine's `IndexWriter` to an in-memory directory, and compares a dump of the result, in `SegmentDump.java`'s form, with the recording (AC-1, AC-2, AC-4); AC-3 is the `ids.json` fixture. The recordings were failing tests before the code existed. `crates/writer/tests/lucene_fields.rs` commits several segments and reopens the writer (AC-5). Unit tests in `lucene/` cover the encodings at their edges (`intToByte4`, sortable floats, `Uid.encodeId`).

## S3 request budget

Unchanged: building a document touches no blob.

## Switch

None: a document is indexed as the Java indexes it, and an index outside the subset stays with a Java writer.

## Risks

- A field the Java adds that the port does not (or the reverse) changes what a search matches: the fixtures set every indexing parameter on and off, and compare every document's every field (AC-2).
- Field numbers that differ from the Java's make segments differ in their field infos, which a merge across the two writers would have to reconcile: AC-1 compares numbers, not just names.
