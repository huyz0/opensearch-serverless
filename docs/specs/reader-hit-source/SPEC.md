# Reader: _source filtering and track_total_hits

## Serves

Parity task R10.4 (hit features), first part; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves the search body's `_source` and `track_total_hits`, ported at OpenSearch 3.10.0 from these Java sources:

- `_source`: `FetchSourceContext.fromXContent`, and `XContentMapValues.filter` with both its set-based and its automaton filters (`crates/reader/src/fetch/source_filter.rs`).
- `track_total_hits`: `SearchSourceBuilder.parseXContent` and `SearchPhaseController.TopDocsStats` (`dsl/source.rs`, `search.rs`, `response.rs`).

`_source` behaves as follows:

- `false` drops each hit's `_source`.
- With no includes or excludes, the stored source is copied as before.
- Otherwise the source is read and written back as OpenSearch reads and writes it:
  - every object's keys come out in `java.util.HashMap` order;
  - integers are written as written, other numbers as `Double.toString` prints them;
  - strings are escaped as Jackson escapes them.

`track_total_hits` takes these values:

- `true` counts exactly.
- `false` or `-1` drops `hits.total`.
- A number N counts exactly up to N, and reports `gte` past it.

Not in this change:

- `docvalue_fields`, `search_after`, `explain`, `highlight` and `stored_fields` come in later changes.
- A `track_total_hits` below -1 is refused as not supported yet. OpenSearch accepts it at parse time and then fails the fetch phase with a body we do not reproduce.
- A hash bucket of more than eight keys in a table of 64 or more, which Java turns into a tree, is not modelled.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/paging-sort.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. That is the existing steps plus 26 new ones:
  - `_source` as `false`, a string, a list, a pattern, includes with excludes, an empty list, no match, and with a sort;
  - `track_total_hits` as `true`, `false`, `-1`, a string, numbers below and above the count, and with a sort;
  - their parse errors.
- AC-2: for one document with nested objects, arrays of objects and of arrays, dotted keys, colliding hashes, negative zero, exponents and large and small doubles, each of 14 `_source` filters writes exactly the bytes stock OpenSearch 3.10.0 wrote, key order included. This matters because the replay compares bodies without regard to key order.
- AC-3: `_source` gives OpenSearch's message for an entry both included and excluded, a non-boolean, non-string, non-array, non-object value, an unknown key, and a non-string list element.
- AC-4: the key order follows Java's `HashMap`, including the resize that a ninth key in one bucket of a table under 64 causes, and agrees with `aggs::metric::hash_order` on every prefix of 200 keys.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2, AC-3 and AC-4: `fetch/source_filter_tests.rs`.

## S3 request budget

Unchanged. Filtering works on the stored source the fetch phase already reads, and `track_total_hits` changes how far the collector counts, not what it reads. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

Filtering one hit's source holds its recorded tokens, a value tree and the orderings, bounded by `scratch_bytes`, which is 64 bytes for each stored byte. The fetch reserves the most any hit in the window needs once, not per hit, and the recorder refuses a source past that bound.

## Switch

None. A key the reader refused before is now answered as OpenSearch answers it, so there is no downside to measure. A whole `_source` is still copied byte for byte.

## Risks

`Double.toString` and Jackson's escaping are matched by tests, not by a Java run on every input. A source whose numbers carry more digits than a double holds is printed as Java rounds it.
