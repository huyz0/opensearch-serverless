# `timeout` in the `_search` body

## Serves

The "every other search key OpenSearch knows" refusal of [compatibility](../../compatibility.md), for the one key clients send on every search: a body with `"timeout":"30s"` answered 400 "the search request's [timeout] is not supported by this node yet", while the URL's `timeout` was accepted ([search-url-params](../search-url-params/SPEC.md)).

## Change

- A body `timeout` that `TimeValue.parseTimeValue` (libs/core/src/main/java/org/opensearch/common/unit/TimeValue.java at 3.10.0) reads is accepted and changes nothing: a number, a fraction allowed, with the unit `nanos`, `micros`, `ms`, `s`, `m`, `h` or `d` (any case, spaces trimmed), or `-1` or `0` with no unit, as a string or a number. The search is not cut short and `timed_out` stays `false`.
- Any other text, and a boolean, is refused as "not supported by this node yet": a unit-less `30`, `soon`, a negative other than `-1`. Java answers its own parse error for these; the exact text is not recorded, so none is claimed.
- A null and an object or array answer the "Unknown key" error of every other value key.
- Not here: `timeout` on `_msearch` item bodies (they take this same parser) and on `_count` (which has no such key).

## Acceptance criteria

- AC-1: each valid form above is read and the search answers as without it.
- AC-2: each form refused above answers 400 "not supported by this node yet"; a null is "Unknown key for a VALUE_NULL in [timeout]." and an object "Unknown key for a START_OBJECT in [timeout]."

## S3 request budget

None added: nothing is read.

## Switch

None: a refusal becomes an answer, and the key changes nothing.

## Tests

The reader's `source` unit tests, and `crates/node/tests/it/search_body_keys.rs` through HTTP.
