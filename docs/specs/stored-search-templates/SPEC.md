# Stored search templates

## Serves

R14.5 and R10: [search-template-api](../search-template-api/SPEC.md) refused a stored template `id`. This lets `_search/template`, `_render/template/{id}` and `_msearch/template` run a mustache script stored with `PUT /_scripts/{id}` ([stored-script-routes](../stored-script-routes/SPEC.md)), as stock OpenSearch's `TransportSearchTemplateAction` does.

## Change

A search service gains a `TemplateSource`, the seam `PipelineSource` is for pipelines: it hands back the mustache source stored under an id. The node's implementation reads the stored-script document set through a `DocSetCache` snapshot, so a warm node pays one marker GET for a request that names an id and nothing for one that does not. The source then renders and runs as an inline template does; nothing else about rendering, the search or the errors changes.

Stock's parse rules carry over: `id` and `source` in one body are two ways to say the template and the later one in the body wins; `/_render/template/{id}` takes the id from the path, whatever the body names.

Not here: a stored script whose `source` was an object (the script routes store a string), `profile`, a `search_pipeline`, a stored template in `_rank_eval`.

## Acceptance criteria

- AC-1: `_search/template` with `{"id":"t","params":{..}}` over a stored mustache script answers the hits the same search with the template inline answers, over an index of one shard and of two.
- AC-2: `_render/template/{id}` answers `{"template_output":..}` for the stored script with the body's `params`; the path id wins over an `id` or `source` in the body; `GET` and `POST` both.
- AC-3: an `_msearch/template` item naming an `id` is searched; an item whose id is missing answers that item's error and the other items still run.
- AC-4: an id nothing is stored under answers 404 `resource_not_found_exception` `unable to find script [t] in cluster state`; a stored script of another language answers 400 `illegal_argument_exception` naming it (stock runs it in its own engine: a **Differs**).
- AC-5: a stored template that does not compile answers 400 `script_exception` with `script` the stored source and `lang` `mustache`, as an inline one does.
- AC-6: `id` and `source` in one body: the later wins, as stock's parser does.
- AC-7: a stored script changed after a request is read on the next one, and a deleted one is then missing (the cache follows the marker).
- AC-8: a set that cannot be read answers 503; a request-body pool that cannot hold the read's buffers answers the 429 a search gives.
- AC-9: a request that names an id on a warm node adds one GET (the marker); an inline template adds none.

## Test plan

Unit: the request records which of `id` and `source` came last (`crates/reader/src/template/tests.rs`). Integration: `crates/node/tests/server/stored_search_template.rs` over a stored-script set and an index, each criterion; the budget rows through `oss_testkit::budget`.

## S3 request budget

`search_template.stored.requests` (one marker GET on a warm node; the first read adds the marker and one GET for each stored script), `search_template.inline.requests` (0 beyond the search). No LIST; nothing scales with indices or elapsed time. Memory: the read's two buffers (`MAX_DOC_BYTES` each) are reserved from the request-body pool before the read, as for a pipeline.

## Switch

None: the request was refused before, so no request that worked changes, and a request that names no id costs nothing more.

## Risks

- A stored set of up to 1,000 scripts is read into the cache on first use (at most 8 MiB kept); a node that reads stored templates but few scripts pays more than it needs. The same shape as search pipelines, accepted there.
- A painless stored script named as a template is refused here and run by stock.
