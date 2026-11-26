# Reader: nested query

## Serves

Parity task R10.1 (queries), the `nested` query; invariant I-13 (REST matches OpenSearch); ADR 0004 (bounded memory). First of three slices of nested support: the reader (this), then the mapping and child documents in `oss-doc` and `oss-writer`, then `inner_hits` and the `nested` and `reverse_nested` aggregations.

## Change

`oss-reader` reads `type: nested` objects out of a mapping and serves the `nested` query over segments that hold block-joined documents (children first, the parent last, as Lucene's `IndexWriter.addDocuments` writes them), from `NestedQueryBuilder.fromXContent` and `doToQuery` at OpenSearch 3.10.0 (`dsl/queries/nested.rs` parses, `nested_build.rs` builds).

- A `nested` object is a path, not a field: `Mapping::is_nested`, `has_nested` and `nested_parent` answer for it, and its sub-fields keep their full names (`c.w`).
- The query is a `ToParentBlockJoinQuery` over the engine's block join. Its children are the inner query filtered to `_nested_path:<path>`; its parents are the documents with a `_primary_term` (`Queries.newNonNestedFilter`) for a top-level path, and the documents of the enclosing nested object (`_nested_path:<parent>`) for a path inside another.
- `score_mode` is `avg` (default), `none`, `min`, `max` or `sum`; `ignore_unmapped` makes an unmapped path match nothing, else it fails `illegal_state_exception` "[nested] failed to find nested object under path [p]"; `path` and `query` are required (`illegal_argument_exception`).
- On an index whose mapping has a nested object, a search runs over the documents that are not nested (`DefaultSearchContext.buildFilteredQuery`), so a match-all, a total, a count or a plain query on a child's field never returns a child.
- No index has a nested mapping yet: the writer still refuses the type, so this is exercised by test indexes built through the engine. Nothing changes for an index without one (no clause is added).

Not here, refused as not supported yet: `inner_hits`, `_name`, a `knn` query on an index with nested objects (it would rank children), nested sorts, nested aggregations.

## Acceptance criteria

- AC-1: the parser reads `path`, `query`, `score_mode` (default `avg`; `none`, `min`, `max`, `avg`, `sum`), `ignore_unmapped` and `boost`, and fails as OpenSearch fails: a missing `path` or `query`, an unknown score mode, an unknown key.
- AC-2: `inner_hits` and `_name` are refused with the node's one message.
- AC-3: the mapping reader records nested paths, finds the enclosing nested object of a nested path, and keeps nested objects out of the fields.
- AC-4: a nested query returns the parents of the children its inner query matches, the total counting parents, for one and several matches and none.
- AC-5: `score_mode` sets the parent's score: for two matching constant-score children `none` 0, `sum` 2, `avg`, `max` and `min` 1.
- AC-6: a nested query composes inside `bool` (`must`, `must_not`).
- AC-7: an unmapped path fails unless `ignore_unmapped`, and an index without nested objects fails the same.
- AC-8: on an index with nested objects, `match_all` returns only parents with the total counting parents, and a term on a child's field returns nothing; an index without nested objects is searched as before.

## Test plan

- AC-1, AC-2: `dsl/queries/nested_tests.rs`.
- AC-3: `mapping_tests.rs`.
- AC-4 to AC-8: `search_nested_tests.rs`, over an index built through the engine by `oss-testkit` (`Spec::with_nested`: two children and a parent a document). AC-8's first test fails with the skip removed (18 documents counted, not 6).

## S3 request budget

Unchanged: the query reads the segments the search already reads, plus the `_nested_path` terms and the `_primary_term` doc values of the same segments. LIST 0, HEAD 0, PUT 0, no new GET. Idle: 0.

## Memory

A parent bit set a segment, one bit a document, built once per segment for each nested query of a search (`QueryBitSetProducer`) and dropped with it. Not yet reserved from the request pool: a segment's bit set is `max_doc / 8` bytes, so the bound is the index's size over 8, and reserving it is the next slice's work (NOT-RUN).

## Switch

None: an index without a nested object is searched exactly as before, and an index with one cannot yet be written.

## Risks

- The block join assumes blocks are intact in a segment. The writer slice must index a document and its children in one call, and a merge must keep blocks together (the engine does).
- OpenSearch's recordings for `nested` are not yet in the replay: it needs an index with a nested mapping, which waits for the writer slice. Until then the build is read from the Java, not recorded.
