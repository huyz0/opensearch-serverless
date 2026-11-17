# Reader: search_after

## Serves

Parity task R10.4 (hit features), second part; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves the search body's `search_after`: a sorted page starts after the document whose sort values the request gives. It is ported at OpenSearch 3.10.0 from `SearchAfterBuilder.fromXContent`, `buildFieldDoc` and `convertValueFromSortType`, and from the checks `SearchService.parseSource` makes first (`crates/reader/src/dsl/search_after.rs`, wired in `dsl/source.rs` and `search.rs`).

- Values are read as Jackson types them: an int, a long, a double, a big integer kept as its text, a string, a boolean or null.
- Each value converts to its sort key's type as Java converts it:
  - `_doc` and integer fields take `intValue`, which wraps a long and truncates a double, or `Integer.parseInt` of a string;
  - `_score` takes `floatValue`, or `Float.parseFloat`;
  - long fields take `longValue`, or `DocValueFormat.RAW.parseLong`, which rounds a decimal down;
  - date fields take `longValue`, or the date format's parser, with `now` refused as OpenSearch refuses it;
  - keyword fields take the value's `toString`, and null sorts as a missing value.
- The page starts after every document equal to the values, as `FieldDoc(Integer.MAX_VALUE, ...)` does.
- Errors follow OpenSearch: `from` above 0 (500 `search_exception`), no sort or a sort by score alone, a count that differs from the sort's, a value that does not parse (400 with its `number_format_exception` or date `parse_exception`), and a null for a numeric key (500 `null_pointer_exception`, as Lucene's comparator fails).

Not in this change:

- Date math other than `now` in a date key's value (`2002-01-01||+1d`) is refused as not supported, as it is in a range.
- `Integer.parseInt` and `Long.parseLong` accept non-ASCII Unicode digits; the reader accepts ASCII digits only. A hexadecimal float string for a `_score` key fails to parse here where Java reads it.
- When `from` is above 0 and `from + size` is also over the result window, the reader reports the window, OpenSearch the `from`.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/paging-sort.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. That is the existing steps plus 33 new ones: `search_after` by integer, two keys, score, keyword, date and `_doc` sorts, with numbers, strings, booleans and nulls of each type, without a total, and every error above.
- AC-2: values parse with Jackson's types, a big integer kept as text, and the empty array, a non-array and a non-scalar element fail with OpenSearch's messages.
- AC-3: each key type converts ints, longs, doubles, strings and booleans as Java converts them, including wrapping, truncation, saturation, `NaN`, whitespace and type suffixes, and the messages of a failed parse.
- AC-4: a null for a numeric key fails with the Java class its unboxing names, after every other value has converted; a keyword key's null is a missing term.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2 to AC-4: `dsl/search_after_tests.rs`.

## S3 request budget

Unchanged. The page start changes which hits the collector keeps, not what it reads. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

The values are held as parsed, bounded by the body's recording; the page start holds one long, and for a keyword key one term, a sort key, at most `MAX_SORT_KEYS`.

## Switch

None. A key the reader refused before is now answered as OpenSearch answers it, so there is no downside to measure.

## Risks

The fixture's replay serializes bodies through a JSON map that orders keys and reads numbers as doubles, so a big integer and an error location after `sort` are checked by unit tests rather than by the replay.
