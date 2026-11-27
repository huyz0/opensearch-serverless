# Learning to rank, native: feature store, models, `sltr` rescore, `ltr_log`

## Serves

R15.4's remaining plugin, under [ADR 0071](../../adr/0071-learning-to-rank-is-ported-natively.md), which supersedes [ADR 0046](../../adr/0046-plugin-requests-are-proxied-to-java-nodes.md) for LTR. The owner's decision (2026-11-27): LTR is ported to Rust, not proxied. A client of the stock plugin (`o19s/opensearch-learning-to-rank-base`) must work unchanged against a Rust node.

## Change

Five slices, one PR each, each behind the setting below.

1. **Evaluators.** `oss_reader::ltr::model`: a model's `definition` parsed once into an immutable evaluator, `score(&[f32]) -> f32`. Types: `model/linear` (weights by feature name), `model/xgboost+json` (gbtree dump: `learner.gradient_booster.model.trees`, split conditions `<`, missing direction, objective `reg:squarederror`, `reg:logistic` and `binary:logistic` with the sigmoid), `model/ranklib` (the RankLib ensemble XML of MART, LambdaMART and Random Forest, with `<split>` thresholds and tree weights; other RankLib rankers are refused). Parsing bounds: definition at most 10 MiB, 4,096 trees, depth 64, 10,000 features.
2. **Feature store and routes.** A store holds feature sets (an ordered list of `{name, params, template_language: mustache, template}`; `template` is a query object or a string) and models (`{name, model:{type, definition}, feature_set}`). Routes of the stock plugin, on the default store and on `/_ltr/{store}/...`: `PUT|POST /_ltr` (create the store), `DELETE /_ltr`, `GET /_ltr` (list stores), `PUT|POST|GET|DELETE /_ltr/_featureset/{name}`, `POST /_ltr/_featureset/{name}/_addfeatures`, `POST /_ltr/_featureset/{name}/_createmodel`, `PUT|POST|GET|DELETE /_ltr/_model/{name}`, `GET /_ltr/_featureset`, `GET /_ltr/_model`, `POST /_ltr/_clearcache`, `GET /_ltr/_stats`. A marker holds at most 10,000 members (a put past it is 400) so it stays under 1 MiB. The default store is named `_default_`, and `/_plugins/_ltr` is the same tree as `/_ltr`. A store, feature set or model is a blob under `ltr/<store>/`, written by compare-and-swap with a version, never listed (a marker per store names its members, as stored scripts do). The definition is read whole only after its bytes are reserved. Validation on put as the plugin does: a model's features exist in its feature set, the definition parses, names are non-empty and not `_`-prefixed.
3. **`sltr` rescore.** `{"sltr":{"model":"<name>","params":{...},"active_features":[...]}}` as `rescore_query` of a `rescore` (and as a plain query, scoring as the model). Per rescored hit: each feature's template is rendered with `params` (the Mustache renderer the search template path uses), parsed as a query, and scored against the candidate; a feature that does not match is its `default` of 0; the model scores the vector. A node caches the resolved model and feature set by `(store, name, version)`. The store's marker lists each member with its version and, for a model, its feature set, so a cold resolution is two rounds: the marker, then the model and its feature set together. Freshness is by TTL, lazily: the first search after an entry's age passes `serverless.ltr.cache_ttl` (default 30 s) answers from the cached entry and starts one background GET of the marker for that store (at most one in flight per store per node); a changed member version reloads that member. No timer and no poll: an idle store makes no request.
4. **`ltr_log`.** `"ext":{"ltr_log":{"log_specs":{"name":"<log>","named_query":"<q>","rescore_index":n,"missing_as_zero":bool}}}` with a `{"sltr":{"_name":"<q>","featureset":"<name>","params":{...}}}` in a `filter` or `should` clause; each hit carries `fields._ltrlog: [{"<log>": [{"name","value"}...]}]`. A section of `ext` that is `ltr_log` stops being refused; `_name` on `sltr` alone is accepted (named queries elsewhere stay refused).
5. **Stats and cache.** `GET /_ltr/_stats` (store status, cache hit and miss counts, request counts) and `_clearcache` for a store.

Not here: RankLib rankers other than MART, LambdaMART and Random Forest; `model/xgboost` (the legacy text dump) and `xgboost+ubj`; training; Java-shell interop. The proxy's `/_plugins/_ltr` and `/_ltr` prefixes are removed when slice 2 lands (the native routes beat a prefix route, but the list should say what the node serves).

## Acceptance criteria

- AC-1: a `model/linear` model scores the dot product of its weights with the feature vector; an unknown feature name in the definition is refused at put.
- AC-2: an XGBoost JSON gbtree model, each of its three objectives, scores as the same model run by XGBoost does, on a fixed set of vectors with recorded scores, including a missing value following the default direction.
- AC-3: a RankLib MART, LambdaMART and Random Forest model scores as RankLib does on recorded vectors; another ranker type is refused 400 at put.
- AC-4: a definition past a bound (size, trees, depth, features) is refused 400 at put, and a malformed one 400 with the parse position, never a panic.
- AC-5: a feature set, a model and a store are created, read, replaced, listed and deleted through the routes with the response shapes written down in `shapes.md` beside this spec (taken from the plugin's published examples, so provisional: `_id`, `_version`, `result`, `found`, `_source`); a missing one is 404 with `found:false`; an invalid one is 400 with the plugin's `illegal_argument_exception` reason.
- AC-6: a put costs 1 GET and 2 PUTs, a read 2 GETs, a list is served from the marker, and none lists (the stored-scripts shape).
- AC-7: `_createmodel` and `_addfeatures` change the stored objects as the plugin does and bump the version.
- AC-8: a `rescore` with an `sltr` query re-orders the window by the model's score, the score a hit gets equals the model applied to the vector the feature queries give, and hits outside the window keep their first-pass order.
- AC-9: a feature whose query does not match a hit gives 0; `params` reach the Mustache template; a template that does not render or parse refuses the search 400 naming the feature.
- AC-10: with a mocked clock, a model replaced through another node is not seen before `cache_ttl` (30 s default) has passed and is seen by the first search after the background refresh; a warm search makes 0 object-store requests; a cold resolution makes 3 GETs in 2 rounds (marker, then model and feature set); the steady bill is at most (stores used) x (nodes) / `cache_ttl` GETs, whatever the search rate, asserted by the request meter over 1,000 searches; a refresh that finds a changed version reloads that member (1 or 2 more GETs), charged per version change, not per TTL window.
- AC-11: `ltr_log` returns each logged hit's feature `{name, value}` in `_ltrlog` in feature set order, with `missing_as_zero` as set; an unknown `named_query` is 400.
- AC-12: a search with `sltr` against an index without the store or model is 400 `resource_not_found_exception`, never a 500; with the setting off every route answers as today (400 `no handler found`) and `sltr` as an unknown query.
- AC-13: memory is reserved before a model is held, charged at 4 x its definition's bytes: a model past the pool's remaining bytes (or `serverless.ltr.cache_bytes`, default 64 MiB) is refused 429 at load, and the evaluators allocate nothing per scored hit, asserted by the counting allocator.

## Test plan

Slice 1: unit tests over small hand-built models and fixtures of recorded XGBoost and RankLib scores (a fixture is generated once from the reference tools and committed with the command that made it, so AC-2 and AC-3 are checked against the tools and not against this code); red first by a stub evaluator returning 0. Slice 2: `crates/node/tests/server/ltr_routes.rs` through HTTP with the request meter for AC-6. Slice 3 and 4: `crates/node/tests/it/ltr_rescore.rs`, `ltr_log.rs` on a small index with known term scores. Each test must fail before its code exists.

## S3 request budget

Management calls: the stored-scripts shape (put 1 GET and 2 PUTs, read 2 GETs, no LIST), a row each in [request-budgets](../../request-budgets.md) when the routes land. Search: 0 requests on a warm node; a cold resolution is 3 GETs in 2 rounds; a refresh is 1 GET of the marker per store per node per `cache_ttl` after a search used it, so at most (stores used) x (nodes) / `cache_ttl`, scaling with nodes and bytes and not with searches or elapsed time per index. Idle indices and idle stores make no request.

## Switch

`serverless.ltr.enabled`, default off. It flips on only when the measurements are recorded in VERIFIED.md: a search's latency with and without `sltr` at a 1,000-hit window and 50 features, the cache's bytes, and the request counts of AC-6 and AC-10; if the rescore costs more than a plain search by more than its own work, the default stays off. Slice 3 ships with `serverless.ltr.max_window` (default 1,000, the most hits an `sltr` rescore scores) and `serverless.ltr.cache_ttl` and `serverless.ltr.cache_bytes`, which the cache reserves from the open-indexes pool.

## Risks

- Reference behaviour comes from the plugin's published documentation and from the reference tools' output, not from its source: `o19s/opensearch-learning-to-rank-base` is outside this sandbox's repositories. Every plugin-visible shape is provisional until a recording from a stock run exists.
- RankLib and XGBoost numeric edge cases (float32 accumulation order, sigmoid clamping) can differ in the last bits; criteria compare within 1e-6 relative.
- Feature queries run per candidate: a window of 1,000 hits and 50 features is 50,000 query evaluations a search; the first slice measures it and the cap is a setting if it hurts.
