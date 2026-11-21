# Reader: track_scores

## Serves

Parity task R10.4 (hit features); invariant I-13. The search body's `track_scores` was refused as not supported yet. lucene-rust already holds the max-score collector (`search_sorted_min_score`'s `track_max_score`); this wires it and scores the hits.

## Change

`oss-reader` parses `track_scores` as a boolean (`SearchSourceBuilder.parseXContent`; a null is an error) and, for a search with a field sort:

- runs a `MaxScoreCollector` beside the sort's collector, so `hits.max_score` is the best score of every match (not just the window) instead of null;
- scores each hit of the window, so `_score` is the number rather than null (`TopFieldCollector.populateScores`). The score is the one the engine's explanation reports for the document, which is the score of the plain search: one explanation a hit, so the work scales with the window, not with the matches.

A search by score is unchanged (it already scores everything). A sort that has the score as a key but not first keeps its score value and gets the tracked best.

Refused (400, "not supported by this node yet"): `track_scores` with a sort led by the score (OpenSearch reports the best score from the first hit there, which is not checked here), and with a field sort over a query this node cannot explain (a range or other points query, `exists`, `ids`, a boosted `constant_score`), since the hits' scores come from the explanation.

## Acceptance criteria

- AC-1: without the key a field sort's hits have a null `_score` and `max_score` is null; with `track_scores: false` the same.
- AC-2: with `true`, each hit's `_score` equals the score of the same document in the search by score, and `max_score` equals the best of those.
- AC-3: `track_scores: true` on a search by score changes nothing.
- AC-4: a sort led by the score, a null value, and a field sort over a range query are 400.

## Test plan

`crates/node/tests/it/search_hits.rs`: `track_scores_scores_the_hits_of_a_field_sort_and_tracks_the_best`, a differential check against the search by score over `compat-term`. Seen failing with the key not passed to the collectors (`max_score` stayed null). Error bodies are not recorded from OpenSearch, so they are provisional.

## S3 request budget

Unchanged: the same segments and norms; scoring a hit reads its postings through the page cache. LIST 0, HEAD 0, PUT 0.

## Risks

- An explanation is slower than a collected score; it runs only for a field sort with the key, one a hit.
