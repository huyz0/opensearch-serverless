# `_msearch/template`

## Serves

Parity plan R10.5, the follow-up [search-template-api](../search-template-api/SPEC.md) left: `GET|POST /_msearch/template` and `/{index}/_msearch/template` run the template requests of a newline-delimited body, a header line then a template request line each, as stock OpenSearch's `RestMultiSearchTemplateAction` does for an inline template.

## Change

`search_msearch` registers the two routes beside `_msearch`'s. The same line streamer reads the header and body lines; a body line of these routes is parsed as a template request, rendered by `oss_reader::template` (the checks `_search/template` makes: no stored `id`, `profile` or `search_pipeline`; a missing template is the stock 400), and the text is held in the request-body pool while it is parsed as a search body. The search then runs as an `_msearch` item runs, and the answer is `{"responses":[...],"took":N}` with the item's own response bytes and `"status":200`, or its error body carrying its status. One failing item never fails the others, and a header or body line that does not parse fails its own item as `_msearch` does. Both routes are `all_access` in the security route table, as `_msearch` is.

## Acceptance criteria

- AC-1: an item's answer equals the same search's answer through `_search`, with `"status":200`, for an index of one shard and of two.
- AC-2: an item whose template does not compile answers the 400 `script_exception` of `_search/template` in its slot, and the items around it are answered.
- AC-3: an item with no template, a stored `id`, a `profile` or a `search_pipeline` answers the same 400 `_search/template` does, in its slot.
- AC-4: the path's index is the default of an item with no `index`; the header rules of `_msearch` apply (one concrete name).
- AC-5: a body with no item answers the 400 `no requests added`; a header with no body line and a missing final newline answer `_msearch`'s errors.
- AC-6: a warm index adds no store request beyond its searches'.
- AC-7: `/_msearch/template` and `/{index}/_msearch/template` need `all_access` as `/_msearch` does.

## Test plan

`crates/node/tests/it/msearch_template.rs` over the compat blobs through HTTP; AC-7 joins the list of `routes_whose_indices_are_in_the_body_or_that_are_proxied_need_all_access` in crates/node/tests/server/security_authz.rs.

## S3 request budget

None of its own: each item costs what its search costs. LIST 0.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- A template whose render differs from Java's text in a runtime failure: as in search-template-api.
