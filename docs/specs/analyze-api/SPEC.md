# `_analyze`

## Serves

Parity plan R10: the stock search-adjacent REST APIs. `GET|POST /_analyze` and `/{index}/_analyze` answer the tokens an analyzer makes of a text, as stock OpenSearch's `RestAnalyzeAction` and `TransportAnalyzeAction.simpleAnalyze` do for the body keys in scope.

## Change

`oss-node` (feature `reader`) gains `search_analyze`: the four routes, registered by one line in `search::routes`, and `SearchService::handle_analyze`. The body is `{"text": "..." | ["...", ...], "analyzer": "standard" | "keyword"}`; the analyzer defaults to `standard`. The answer is `{"tokens":[{"token","start_offset","end_offset","type","position"[,"positionLength"]}...]}` where, as the Java does, positions count from 0 and offsets and positions carry across the texts of an array with the analyzer's gaps: an offset gap of 1, and a position gap of 100 when the request names an index (its analyzers carry the mapping default) and 0 when it does not (a global analyzer is Lucene's own). An index in the path must exist (404 as a search answers it); it changes the gap and nothing else, since this node holds no custom analyzers.

More than `index.analyze.max_token_count` (10,000, the default; not settable here) tokens answers the Java's `IllegalStateException`, a 500 `illegal_state_exception` with the Java's message.

Not here: `field`, `tokenizer`, `filter`, `char_filter`, `normalizer`, `explain` and `attributes` (each a 400 `illegal_argument_exception` "is not supported by this node yet"), analyzers other than `standard` and `keyword` (the same for the built-in names this node lacks, `failed to find [global ]analyzer` for the rest), and every URL parameter.

## Acceptance criteria

- AC-1: with no analyzer, the standard analyzer's tokens of a text: lower-cased words with their offsets, `<ALPHANUM>` and `<NUM>` types, and positions from 0.
- AC-2: `"analyzer":"keyword"` answers one token of the whole text, type `word`.
- AC-3: an array of texts continues positions and offsets across the texts: gap 0 without an index, 100 with one, offsets by the previous text's end plus 1.
- AC-4: a missing, empty or `[]` `text` answers 400 `action_request_validation_exception` "Validation Failed: 1: text is missing;".
- AC-5: an unknown analyzer answers 400 `illegal_argument_exception` "failed to find global analyzer [x]" (with an index, "failed to find analyzer [x]"); a built-in this node lacks and every unsupported key answer "... is not supported by this node yet".
- AC-6: an unknown index answers the 404 `index_not_found_exception` a search does; an unknown URL parameter answers the router's 400.
- AC-7: more than 10,000 tokens answers the 500 `illegal_state_exception` naming the limit.
- AC-8: `GET` and `POST` answer alike, and a call over a warm open index makes no object-store request; a call with no index makes none at all.

## Test plan

`crates/node/tests/it/analyze.rs` over the compat blobs, through HTTP, the expected tokens written by hand from the Lucene analysis rules. AC-7 reads the meter as `it/count.rs` does.

## S3 request budget

None for a call without an index; the search's open for one with an index. LIST 0.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- The response is built before it is written, so a too-long result can answer a status; its tokens are bounded by the 10,000 limit and each token's bytes are reserved from the request-body pool as it is added.
