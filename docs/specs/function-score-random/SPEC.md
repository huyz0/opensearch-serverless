# `function_score`'s `random_score` and `min_score`

## Serves

The first two entries of `docs/compatibility.md`'s `function_score` refusal list. Stock `function_score` takes `random_score` (`RandomScoreFunctionBuilder`) as a function and `min_score` (`FunctionScoreQueryBuilder`) beside its functions. The engine already runs both (`Function::Random`, `FunctionScoreQuery::with_min_score`); the reader refused them at parse time.

## Change

- `random_score` parses as a function, alone or in `functions`, under the same `filter`, `weight` and modes as the others. Its keys are `seed` (an integer, a long or a string), `field` and nothing else; `_name` is refused like every `_name`.
- No `seed`: the score is index-wide document id based, seeded from the request time (`Long.hashCode(now)`), as stock. With a `seed`: the field is `field`, else `_id`; a keyword hashes its first term, an integer, long or date its decimal text, `_id` its id. A text field is the stock fielddata 400; a field not in a non-empty mapping is the stock "Field [f] is not mapped on [[index/uuid]] and cannot be used as a source of random numbers." 400; an empty mapping scores by document id, as stock.
- The salt is `(String.hashCode(index) << 10) | shard`, mixed into the seed with the engine's `BitMixer.mix`. An int seed is used as is, a long seed is `Long.hashCode`, a string seed is `String.hashCode` (UTF-16 units).
- A seed that is a float, a number above a long, a boolean or null is stock's `parsing_exception`.
- `min_score` (a number) drops every document whose function score is below it, with no functions too (the sub-query then scores as stock: unchanged).
- No new setting: both only add accepted shapes, with no measured downside.

## Acceptance criteria

- AC-1: `random_score` without a seed scores every match in `[0, 1)` and answers, on one request, equal scores for no two matches that stock would hash apart; two requests may order differently.
- AC-2: `random_score` with a `seed` and a `field` returns the same order on every request and the same score for a document wherever it sits in the result.
- AC-3: with `seed` and no `field` the score depends on the document's `_id`; with a numeric, date or keyword `field` on that field.
- AC-4: the random seed's failures are stock's: a float seed, a boolean seed, an unknown key, an unmapped field and a text field.
- AC-5: `min_score` keeps exactly the matches scoring at or above it, with functions, with a bare `weight`, and with none.
- AC-6: `random_score` and `min_score` add no object-store request.

## Tests

Unit: `crates/reader/src/dsl/queries/function_score_tests.rs`, `function_score_build_tests.rs`. Integration: `crates/node/tests/it/function_score_random.rs`.

## S3 request budget

None added: doc values and the stored `_id` are read as for any scored search.
