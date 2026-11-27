# 0073: A stored script named by id resolves through a per-node cache checked once a TTL

**Status:** Accepted after independent review, 2026-11-27

## Context

`script: {"id": "x", "params": {..}}` names a script stored with `PUT /_scripts/x` ([stored-script-routes](../specs/stored-script-routes/SPEC.md)). Every Painless site refuses an `id` today ("send the script as [source]"). Resolving an id is a read of the `scripts/` document set; read naively it would cost one marker GET each time a script is parsed, and a search parses its scripts on the coordinator and again on each shard, so requests would scale with queries times shards ([principles](../principles.md): requests scale with nodes and bytes, idle indices make zero requests). The same shape was solved for learning to rank ([ADR 0071](0071-learning-to-rank-is-ported-natively.md)): a per-node cache whose freshness is a TTL checked lazily by the first use past it.

## Decision

A stored script named by `id` is resolved to its language and source where the script is parsed (the shared Painless script parser of the reader, and the update body parser of the node), through one resolver the node installs in `oss-reader` as it does for LTR models. A search parses its body once, on the node that coordinates it, into one parsed source that every shard's query is built from (a remote shard action carries no script), so every shard of one search runs the one resolved version. `_update_by_query`, `_reindex` and a bulk update resolve once per request or task, before the walk or the apply. The node's resolver keeps one in-memory snapshot of the script set and the time it was last checked:

- Within the TTL (default 10 seconds, `OSS_SCRIPT_CACHE_TTL`, 1 to 3,600, read at boot beside `OSS_LTR_CACHE_TTL`) a resolve is a memory lookup: no request.
- The first resolve past the TTL checks the set's marker (one GET) and re-reads only the documents whose key moved (`DocSetCache`, as for stored templates). Concurrent resolves wait for that one refresh rather than make their own.
- No timer, no poll, no LIST. A node that resolves no script makes no request; the steady bill is at most one marker GET per node per TTL while scripts are in use, independent of search rate and shard count.
- A script change is visible on a node within the TTL. The node that took a `PUT` or `DELETE` sees it at its next resolve: the route bumps a generation counter, and a refresh that began before the bump does not record a check time, so an in-flight refresh cannot hide the change for a TTL. On another node a new script is 404 until its TTL ends.
- One snapshot per node, of the one script set under the node's base (not per tenant), in its own `DocSetCache` of 8 MiB, not shared with stored templates'. Resolves that arrive during a refresh wait on the one refresh, which ends within the store client's own timeout and fails them all with its status.
- The refresh reserves two buffers of `MAX_DOC_BYTES` from the request-body pool before it reads, and the snapshot is bounded by its cache's byte limit. A set past it is refused 429 `circuit_breaking_exception` naming the limit; the way out is to delete scripts. Stored templates share that property today.
- The resolved source is checked as an inline one is: lang `painless` (the update route and search sites that serve expression scripts take `expression` too), at most 65,535 bytes, and the switch `serverless.scripting.painless.native_compile` still governs whether Painless runs. A missing id is 404 `resource_not_found_exception`; an unreadable store is 500. An `id` with `source` or `lang` in the same object is 400.
- No new setting beyond the TTL: the feature adds no request on a path that does not name an id, so there is no downside to switch off.

## Consequences

- Easier: stored scripts work at every Painless site (script_score, script_fields, sort, `script` query, scripted_metric, update, update-by-query, reindex, bulk update) with one resolver.
- Harder: a script edit is stale for up to a TTL on other nodes, as OpenSearch's cluster state would be for a short moment but longer; a script run mid-update may be the old one.
- Rules out: resolving per shard per query, and any per-search store read for a stored script.
