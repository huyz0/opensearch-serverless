# Reader: the search body names a point in time

## Serves

R10.6: searching a point in time. The request half: a `_search` body's `pit` is read so a node can find the view the search runs on. Follows [reader-frozen-view](../reader-frozen-view/SPEC.md); ported from `PointInTimeBuilder`'s parser at OpenSearch 3.10.0.

## Change

`oss-reader`'s search source gains `pit`: `response::write_response_with` writes a response that names the view as `pit_id`, and `SearchSource::pit` is the view's `id` and its optional `keep_alive` text, read from `{"pit":{"id":"...","keep_alive":"1m"}}`. Until now `pit` was refused as not supported. The `keep_alive` text is kept as written; its meaning is the node's.

- `id` is a string and required.
- `keep_alive` is a string; a null is as absent.
- Any other field of `pit` is refused, as is a `pit` that is not an object.
- Not here: finding the view, opening it, extending it or answering `pit_id` (the node's search).

## Acceptance criteria

- AC-1: `{"pit":{"id":"X","keep_alive":"1m"}}` gives the id and keep-alive as written, in any chunking of the body; with no keep-alive it gives none.
- AC-2: a `pit` without an `id`, with an `id` that is not a string, with a field other than `id` and `keep_alive`, or that is not an object is refused with a 400 `parsing_exception`, and a body with no `pit` has none.
- AC-3: a response written for a point in time names the view first as `pit_id`, and is otherwise the response a live search writes, byte for byte.

## Test plan

`crates/reader/src/dsl/source_tests.rs`, and `crates/reader/src/response_hits_tests.rs` for AC-3.

## S3 request budget

None: parsing a body reads nothing.

## Risks

- The refusal messages follow `ObjectParser`'s wording from the Java source and are not recorded from a running OpenSearch.
