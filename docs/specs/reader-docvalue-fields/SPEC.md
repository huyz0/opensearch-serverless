# Reader: docvalue_fields

## Serves

Parity task R10.4 (hit features), third part; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves the search body's `docvalue_fields`: each hit carries a `fields` object with the values of the fields asked for, read from their doc values. It is ported at OpenSearch 3.10.0 from `FieldAndFormat.fromXContent`, the `index.max_docvalue_fields_search` check of `SearchService.parseSource`, and `DocValueFieldsFetchSubPhase` (`crates/reader/src/fetch/docvalue_fields.rs`, wired in `dsl/source.rs`, `search.rs` and `response.rs`).

- An entry is a name, a `*` pattern, or an object with `field` and an optional `format`; a number or boolean entry is read as its text.
- A name or pattern resolves over the mapping; an unmapped one adds nothing.
- Integer and long fields write numbers, date fields the default format (`2001-03-04T00:00:00.000Z`), keyword fields strings, each in doc-values order.
- A field asked for twice writes its values twice.
- The `fields` object comes out in `java.util.HashMap` order and is left out when no field has a value; it sits between `_source` and `sort`.
- Errors follow OpenSearch: more than 100 entries, a text field, a field without doc values (each only when there is a hit to fetch), and every parse error of `FieldAndFormat`.

Not in this change:

- A `format` other than `use_field_mapping` is refused as not supported yet.
- A metadata field (`_id` and the others), and a pattern that starts with `_` or `*` and so could match one, are refused as not supported yet.
- Fields of the types the reader does not read yet (`geo_point` and the others) are refused as not supported yet.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/paging-sort.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. That is the existing steps plus 22 new ones: each field type, a pattern, unmapped names, text fields with and without hits, the object forms, a field twice, with a sort, the limit and every parse error.
- AC-2: the parse errors carry OpenSearch's messages and locations, including the suggestion for a near key.
- AC-3: names and patterns resolve to fields in `java.util.HashMap` order, the order OpenSearch writes them, with a repeated field counted, and every refusal above.
- AC-4: a hit's values are written from its segment's doc values, keywords looked up by ordinal, dates formatted, and nothing written when no field has a value.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2 to AC-4: `fetch/docvalue_fields_tests.rs`.

## S3 request budget

A search without `docvalue_fields` is unchanged. With it, the fetch phase reads each hit's doc values through the page cache, as it reads its stored fields: the reads scale with the bytes of the hits' doc-value pages, never with documents of the index. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

The request holds at most 100 entries; the resolved fields are at most one a mapped field (1,000). Writing one hit holds one field's values for that document at a time, each keyword at most a term's length; the body streams to the writer.

## Switch

None. A key the reader refused before is now answered as OpenSearch answers it, so there is no downside to measure.

## Risks

The fixture's documents are single-valued, so the order of several values of one field is checked against the doc values' order by unit tests, not against a recording.
