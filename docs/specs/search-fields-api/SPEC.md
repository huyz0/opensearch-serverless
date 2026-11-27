# The `fields` API of a search body

## Serves

The `fields` refusal of [compatibility](../../compatibility.md). Kibana's Discover and the official clients ask each search for `"fields":["*"]` or a list of names, and every one answered 400.

## Change

A search body's `fields` array (`FieldAndFormat`: a name, a `*` pattern, or `{"field":..,"format":..}`) makes each hit carry a `fields` object with the values of the fields asked for, read from the hit's `_source`. It is ported from `FieldFetcher`, `FetchFieldsPhase`, `SourceValueFetcher`, `XContentMapValues.extractValue` and the `valueFetcher` of the keyword, text, numeric and date mappers, at the OpenSearch tip the Java shell builds on (`fae98a3`, of the 3.x line).

- A name or `*` pattern is resolved over the mapped fields; an unmapped name, and a metadata field such as `_id`, adds nothing. A field asked for twice is written once.
- A value is looked up at the field's source path (a multi-field such as `title.keyword` reads its parent's path), through objects and arrays of objects; a `null` or missing value adds nothing. Nested arrays are flattened breadth first, as `SourceValueFetcher` does.
- `keyword` and `text` write the source value as `toString` gives it (a number as Java prints it, a boolean as `true`/`false`); a keyword longer than its `ignore_above` (UTF-16 units) is left out.
- `integer`, `short`, `byte` and `long` write a number: an integral number, or a string of one, as it is; a number with a decimal part is truncated towards zero, as `coerce` does; an empty string adds nothing.
- `date` (the default format) writes `yyyy-MM-ddTHH:mm:ss.SSSZ`, read from a date string or epoch milliseconds.
- The `fields` object comes out in `java.util.HashMap` order and is left out when no field has a value; it sits between `_source` and `sort`.
- A `format` on a keyword, text or numeric field fails as OpenSearch does ("Field [x] of type [keyword] doesn't support formats."), only when there is a hit to fetch.

Not in this change, each refused as not supported by this node yet (only when there is a hit to fetch):

- A `format` on a date field.
- A matched field of a type the reader does not read yet, one with `null_value`, `ignore_malformed: true`, `coerce: false`, `index_prefixes` or `index_phrases: true`, and one a `copy_to` names.
- `fields` together with `docvalue_fields`, `script_fields` or `collapse` in one request.
- `fields` inside `top_hits` and `inner_hits`, and a search the Java coordinator forwards.
- A source value the node cannot read as the field's type (text for a number, a malformed date, an object for a keyword) fails the response where OpenSearch fails the shard.

## Acceptance criteria

- AC-1: names and patterns resolve over the mapping, a multi-field reads its parent's path, nested arrays flatten breadth first, and nulls and missing values add nothing.
- AC-2: keyword (with `ignore_above`), text, integer-family, long and date values are written as above, and a long keeps its precision.
- AC-3: the `fields` object is written in `HashMap` order between `_source` and `sort`, and not at all when no field has a value.
- AC-4: the format refusals carry OpenSearch's messages, and every refusal above answers 400 "not supported by this node yet".

## S3 request budget

None added: the values come from the `_source` the hit's fetch already read.

## Switch

None: refusals become answers, and a search without `fields` writes as before.

## Tests

`crates/reader/src/fetch/source_fields_tests.rs` and `crates/node/tests/it/search_fields.rs` through HTTP. No recording from stock OpenSearch exists for these cases (none can be made in the cloud sandbox), so the expected values are derived from the Java source named above and are provisional until a fixture is recorded.
