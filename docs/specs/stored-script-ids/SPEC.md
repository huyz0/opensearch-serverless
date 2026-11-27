# Stored Painless scripts named by id

## Serves

R14.6's stored-script gap: every Painless site refuses `script: {"id": ..}`. Decision: [ADR 0073](../../adr/0073-stored-painless-scripts-resolve-through-a-ttl-cache.md). Builds on [stored-script-routes](../stored-script-routes/SPEC.md), [painless-script-score](../painless-script-score/SPEC.md) and the update specs.

## Change

- `oss-reader`: a resolver trait (`stored_scripts`) the node installs, as `ltr::models` is; the Painless script parser (`function_score_script::object_with`, which `script_score`, `script_fields`, `sort._script`, the `script` query and `scripted_metric` use) resolves an `id` to the stored script's source and takes its `params` from the object. With no resolver installed the `id` stays refused as before.
- `oss-node`: the resolver over the stored-script set with the cache of the ADR; the update body parser (`_update`, `_update_by_query`, `_reindex`, bulk update) resolves an `id` through it.
- Stored scripts of another language than `painless` (`mustache`, `expression` where the site serves only Painless) are 400 naming the language. A missing id is 404 `resource_not_found_exception` ("unable to find script [x]"); a script object with `id` and `source`, or `id` and `lang`, is 400.
- `OSS_SCRIPT_CACHE_TTL` seconds (default 10).

## Acceptance criteria

- AC-1: `script_score` by `id` scores as the same script inline, with its `params`.
- AC-2: `script_fields`, a `_script` sort, the `script` query and a `scripted_metric` script each run a stored script by `id`.
- AC-3: `_update`, `_update_by_query`, `_reindex` and a bulk `update` item run a stored script by `id`.
- AC-4: a missing id is 404, an id with `source` or `lang` 400, a stored `mustache` script 400, and an id with no resolver installed refused as before.
- AC-5: resolving the same id again within the TTL makes no store request; the first resolve past the TTL makes one marker GET and sees a changed script; a node that resolves nothing makes no request.
- AC-6: a script set past the cache bound is refused 429, not held; an unreadable store is 500.
- AC-7: a search over several shards resolves each id once, so every shard runs one version; concurrent resolves past the TTL make one refresh.
- AC-8: a `PUT` or `DELETE` is seen by the node that took it at its next resolve, even when a refresh was in flight; another node sees it within the TTL.
- AC-9: `OSS_SCRIPT_CACHE_TTL` outside 1 to 3,600 stops the node at boot; `docs/compatibility.md` records the stored-script behaviour including the 404 until the TTL ends on another node.

## Test plan

`crates/node/src/stored_script_source.rs` unit tests with a metered emulator and an injected clock; `crates/node/tests/script_score_painless.rs` for the search sites; `crates/node/tests/server/by_query_route.rs`-style rig for the update sites.

## S3 request budget

Zero for a request that names no id. With ids: at most one marker GET per node per TTL while scripts are in use, plus a read of each changed document on refresh; no LIST.

## Bounds

The snapshot is at most the document cache's 8 MiB; a refresh holds two `MAX_DOC_BYTES` buffers reserved from the request-body pool; one refresh at a time.

## Switch

None beyond the TTL (ADR 0073); Painless still runs only with `serverless.scripting.painless.native_compile` on.

## Risks

- A changed script is stale on another node for up to the TTL, and a new one is 404 there until then (OpenSearch's cluster state would show it at once).
- The 404 and the lang-with-id refusal are reproduced from OpenSearch's documented behaviour, not recorded from a stock node, so they stay provisional.
