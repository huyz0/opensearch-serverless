# Reader: terminate_after

## Serves

Parity task R10.4 (search features); invariant I-13. The search body's `terminate_after` was refused as not supported yet. lucene-rust already holds the early-terminating collector (`terminate::search_sorted_until`); this wires it, writes the `terminated_early` flag, and refuses the shapes where the engine's own parity notes say the cut falls elsewhere.

## Change

`oss-reader` parses `terminate_after` as an integer (`SearchSourceBuilder`: a negative is `terminateAfter must be > 0`, a null is an error, 0 is off). With a positive value, a search by score collects only the first `n` live matches of the shard in index order (segment by segment, each in document order) and ranks the window from those:

- the shard's total is the number let through, exact;
- the shard's `terminated_early` is true when a match past the `n`th, or a segment after the one holding it, ended the collection;
- the response writes `"terminated_early":<bool>` right after `"timed_out"`, on one shard and, as the OR of the shards' flags, over several (`SearchPhaseController`); it is absent when the key was not asked.

Refused (400, "[terminate_after] with ... is not supported by this node yet"): a `size` of 0 (its count is `Weight.count` per segment, a different collector), `min_score`, `track_scores`, any sort, `search_after`, aggregations, a `track_total_hits` below `n`, and a query with nothing to score (a filter-only bool, a lone `constant_score` or `match_all`), which Lucene collects in ranges and cuts elsewhere.

## Acceptance criteria

- AC-1: `terminate_after: n` over more than `n` matches keeps the first `n` in index order, ranked by score, with total `n` (exact) and `terminated_early: true`.
- AC-2: over fewer matches it returns the hits of the plain search and `terminated_early: false`; 0 is the plain search with no flag.
- AC-3: the response head is `"timed_out":false,"terminated_early":<bool>,"_shards"`, and over shards the flag is the OR of theirs, absent when no shard says.
- AC-4: a negative or null value, and each refused combination, are 400.

## Test plan

Reader unit tests in `crates/reader/src/search_tests.rs` (the cut, the pass-through, parsing, refusals); `crates/coordinator/tests/search.rs` (the head and the OR); `crates/node/tests/it/search_hits.rs` (end to end over `compat-term`). The expected bodies are read from the Java and lucene-rust's parity notes, not recorded from OpenSearch, so they are provisional.

## S3 request budget

Unchanged: the same segments and norms. LIST 0, HEAD 0, PUT 0.

## Risks

- Whether Lucene's batching cuts at the same document for every scored query is the engine's own claim (lucene-rust `docs/parity/search-collectors.md`), checked there against Java fixtures, not here. The query guard is conservative: a `constant_score` inside a `must` is not detected.
