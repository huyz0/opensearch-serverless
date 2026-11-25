# Search pipeline named in the body, and on `_msearch`

## Serves

R14.6, after [search-pipelines](../search-pipelines/SPEC.md) and [search-default-pipeline](../search-default-pipeline/SPEC.md). In OpenSearch a search body may carry `"search_pipeline": "<id>"` (a name) or an object (an inline pipeline), and each `_msearch` item is a search body, so that is how `_msearch` takes a pipeline; `_count` takes none (it has no `search_pipeline` parameter and its handler builds no pipelined request), so `_count` is unchanged and keeps answering an unrecognized `search_pipeline` parameter 400.

## Change

- `SearchSource::search_pipeline`: a body `"search_pipeline": "<id>"` is read (a string, not empty). An object form, `null`, or any other type stays refused as it is today ("not supported by this node yet"): an inline pipeline needs the definition's own text, which the recorded tokens do not give back without a second parser.
- Which pipeline runs, for a `_search` of one index: `?search_pipeline=` first, then the body's, then the index's `index.search.default_pipeline`; `_none` at either of the first two means none and stops the lookup.
- Each `_msearch` item runs the same way over its own body and its header's index: the body's name, else that index's default. `?search_pipeline=` on `_msearch` is not a parameter of OpenSearch's `_msearch` and stays unrecognized. A pipeline failing in one item is that item's error (its status and body), the other items are answered.
- With search pipelines off, a body that names a pipeline other than `_none` is refused 501 `not_implemented`, as the parameter is.
- A `_msearch` item holds the pipeline buffers (two of `MAX_DOC_BYTES`) from the request-body pool while it runs; the item's reservation is released before the next item.

## Acceptance criteria

- AC-1: A body naming a stored pipeline runs it on `_search`; `?search_pipeline=` overrides the body's; `_none` in the body bypasses the index default.
- AC-2: A body `search_pipeline` that is an object, null or a number is still refused 400 as before.
- AC-3: Each `_msearch` item runs the pipeline its body names, or its index's default, and an item naming a missing pipeline is that item's 400 `pipeline_missing` while the others answer 200.
- AC-4: With search pipelines off a body naming a pipeline is 501, and `_none` is not.
- AC-5: `_count` still refuses the `search_pipeline` parameter as unrecognized.
- AC-6: A `_msearch` of items naming no pipeline makes the same requests as before (the existing `_msearch` budget gates, unchanged), and each item that names one adds one marker GET.
- AC-7: A body naming a `search_pipeline` is refused 400 where no pipeline runs: by-query searches and rendered templates (the reader's `SearchSource::refuse_pipeline`).

## S3 request budget

Unchanged for a search that names no pipeline. An item that names one adds one marker GET on a warm node, once per item (the node's snapshot cache is read per item: one request each when the set is unchanged).

## Switch

`Config::search_pipelines`.

## Risks

A `_msearch` of n items naming a pipeline reads the set marker n times; bounded by the `_msearch` item cap, and a request-scoped snapshot is a follow-up if it shows in a measurement.
