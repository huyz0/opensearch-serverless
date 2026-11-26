# `_render/template` and `_search/template`

## Serves

Parity plan R10: the stock search-adjacent REST APIs. `GET|POST /_render/template` renders an inline mustache template over `params`, and `GET|POST [/{index}]/_search/template` renders it and runs the result as a search body, as stock OpenSearch's `RestRenderSearchTemplateAction` and `RestSearchTemplateAction` (lang-mustache) do.

## Change

`oss-reader` gains `template`: a port of mustache.java 0.9.14 as lang-mustache runs it (compile, the standalone-line rules, sections, inverted sections, `toJson`, `join`, `url`, delimiter changes, partial and extend refusals), the `SearchTemplateRequest` body parser, and a bounded executor. Its rendering is checked against the Java engine by a recorded fixture (`fixtures/mustache/`, 141 cases) replayed in unit tests, and was fuzzed against the live Java oracle while built.

`oss-node` (feature `reader`) gains `search_template`: the routes and `SearchService::handle_render_template` and `handle_search_template`. The rendered text is parsed with the search-body parser and searched through `search_bytes`, the path `_msearch` uses, so a sharded index goes through the coordinator.

`_render/template` answers `{"template_output": <the rendered JSON, as rendered>}`. `_search/template` answers the search response. Request `explain` is applied to the rendered source.

Errors: a template that does not compile is a 400 `script_exception` (`script_stack` `[]`, `script`, `lang` `mustache`, `caused_by` `mustache_exception`, or `mustache_not_found_exception` for a missing partial); an illegal template is a 400 `illegal_argument_exception`; a body that is not a template request, or has no template, is the 400 stock gives, `Validation Failed: 1: template is missing;`. Rendered text that is not a search body gets the error a search body gets.

Not here, refused with 400 "... is not supported by this node yet": `profile`, a `search_pipeline`, `simulate`, partials and extend sections, the mustache members of Java classes this node has no values for, and the URL parameters beyond `size` and `from`. Rendering is capped at 1 MiB, reserved from the request-body pool, and nesting at 512 levels.

## Acceptance criteria

- AC-1: `_render/template` with `source` and `params` answers 200 `{"template_output": ...}` with the Java engine's text for the recorded cases.
- AC-2: `_search/template` renders then searches; its hits equal those of the same `_search` body, over an index of one shard and of two.
- AC-3: `{{#toJson}}`, `{{#join}}` and `{{#url}}` render as Java does.
- AC-4: a template that does not compile answers 400 `script_exception` naming the Java message; an illegal template answers 400 `illegal_argument_exception`.
- AC-5: a body with no template answers 400 `Validation Failed: 1: template is missing;`; a body that is not an object, an unknown key and a wrong-typed value answer the 400 parse errors stock gives.
- AC-6: `profile`, a `search_pipeline` and a template over the cap answer 400 refusals; a stored `id` and `/_render/template/{id}` do too on a node with no stored-script set, and are served by [stored-search-templates](../stored-search-templates/SPEC.md) where there is one.
- AC-7: rendered text that is not a valid search body answers the search error; an unknown index answers the 404 a search does.
- AC-8: a warm open index makes no request beyond the search's own.

## Test plan

Unit: `crates/reader/src/template/tests.rs` replays `fixtures/mustache/render.json` and tests refusals, limits and the request parser. Integration: `crates/node/tests/it/search_template.rs` over the compat blobs through HTTP.

## S3 request budget

None of its own: a render touches no store; a search costs what `_search` costs. LIST 0.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- A runtime failure of a template (a method call Java would make) differs in text from Java's nondeterministic one; documented in compatibility.
- Mustache members of Java classes outside the scalars, lists and maps of JSON are refused rather than emulated.
