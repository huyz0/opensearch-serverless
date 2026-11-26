# `_count` runs the index's default search pipeline

## Serves

The `_count` half of the "`_msearch` and `_count` take no default" gap of [search-default-pipeline](../search-default-pipeline/SPEC.md). `_msearch` items already run their index's default ([search-pipeline-body](../search-pipeline-body/SPEC.md)). Stock `_count` is a search of size 0 (`RestCountAction` calls `client.search`), so `TransportSearchAction` resolves the index's `index.search.default_pipeline` for it as for any search; a `filter_query` request processor there narrows the count. This node ignored the default, so its count was larger than stock's.

## Change

- With search pipelines on, `_count` of one named index with no body or parameter pipeline (`_count` has neither, as stock) runs the pipeline `index.search.default_pipeline` names, over the count's query, before it counts. A default of `_none` or no default runs nothing. A default naming a pipeline that is not stored is 400 `pipeline_missing`, as for a search.
- Only request processors can change a count: a response processor (`truncate_hits`) has nothing to cut in a search of size 0. The count still asks for no hits and a total of its own, after the pipeline ran.
- With search pipelines off the default is ignored and no request is added, as for a search.
- No new setting: this is the switch `Config::search_pipelines` of [search-pipelines](../search-pipelines/SPEC.md).

## Acceptance criteria

- AC-1: `_count` of an index whose default pipeline is a `filter_query` counts only the matches of that filter and the query together; the same `_count` of an index without a default counts all matches.
- AC-2: a default of `_none` runs nothing; a default naming an absent pipeline is 400 `pipeline_missing`.
- AC-3: with search pipelines off a default is ignored and a warm `_count` is free.
- AC-4: with search pipelines on, a warm `_count` of an index with no default makes no request.

## S3 request budget

As a search with a default ([search-default-pipeline](../search-default-pipeline/SPEC.md)): a count that runs a default adds one marker GET on a warm node; one without a default adds none.

## Tests

`crates/node/tests/it/search_pipeline.rs`.
