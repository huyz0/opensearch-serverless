# Reader: version and seq_no_primary_term on search hits

## Serves

Parity task R10.4 (hit features); invariant I-13 (REST matches OpenSearch's behaviour). A client that reads a hit and writes it back conditionally (`if_seq_no`, `if_primary_term`) needs the hit's sequence identity; until now the search body's `version` and `seq_no_primary_term` were refused as not supported yet.

## Change

`oss-reader` serves the two boolean keys of the search body, ported at OpenSearch 3.10.0 from `SearchSourceBuilder.parseXContent`, `FetchVersionPhase`, `SeqNoPrimaryTermPhase` and `SearchHit.toInnerXContent`:

- `version: true` writes each hit's `_version`, from the numeric doc values `_version`.
- `seq_no_primary_term: true` writes `_seq_no` and `_primary_term`, from the doc values of the same names.
- They are written between `_id` and `_score`, in the order `_version`, `_seq_no`, `_primary_term`.
- A document without the doc value carries none of it (OpenSearch's `-1` and unassigned sentinels are not written); `_seq_no` and `_primary_term` are written together or not at all.
- `false` writes nothing; `null` is refused as `explain`'s null is.

Nothing is read unless a key is true, so a search without them makes the requests it made before. With them, the doc values come through the page cache and ranged reads a segment's other doc values do; the hit's stored fields are already fetched.

Not in this change: the `version` and `seq_no_primary_term` of `top_hits`, which stay refused, and the URL parameters of the same names.

## Acceptance criteria

- AC-1: with neither key, or both `false`, a hit has no `_version`, `_seq_no` or `_primary_term`.
- AC-2: `version: true` writes `_version`, `seq_no_primary_term: true` writes `_seq_no` and `_primary_term`, and both write all three, each between `_id` and `_score` in that order, with the values the fixture index holds.
- AC-3: the `_seq_no` and `_primary_term` written are the doc values a `docvalue_fields` request for them reads, and a `null` value for either key is a 400.

## Test plan

`crates/node/tests/it/search.rs`: `a_hit_carries_its_version_and_sequence_identity_when_asked`, over the recorded `compat-term` index (written by OpenSearch, so it holds all three doc values). The expected key order is read from `SearchHit.toInnerXContent`, not recorded from a running OpenSearch, so it is provisional until a fixture step records it.

## S3 request budget

Unchanged without the keys. With them, no new request kind: doc values of segments the search already opened. LIST 0, HEAD 0, PUT 0.

## Risks

- A `_version` written by a writer that did not stamp it would be missing from a hit rather than `-1`; both omit the key, as OpenSearch does.
