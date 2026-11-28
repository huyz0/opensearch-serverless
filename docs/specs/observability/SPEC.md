# Observability objects, native: saved queries, visualisations, panels and notebooks as stored documents

## Serves

The "Observability" row of [java-dependencies](../../java-dependencies.md) and the owner's rule (2026-11-28) that every plugin is native with no Java in the runtime path. Pattern: [LTR](../ltr/SPEC.md) and [ADR 0071](../../adr/0071-learning-to-rank-is-ported-natively.md). [ADR 0086](../../adr/0086-observability-is-ported-natively.md): the store shape of [ADR 0073](../../adr/0073-stored-painless-scripts-resolve-through-a-ttl-cache.md) applied to a new family, with one document-set directory and no process or timer.

## Change

The stock Observability plugin is, on the server, a store of typed documents behind `/_plugins/_observability/object`; the Dashboards Observability app keeps its saved queries, visualisations, operational panels, notebooks and timestamps there. Nothing runs on a schedule and nothing executes a saved query: the PPL a saved query holds is run by the client through `/_plugins/_ppl`, which is a different row (not served, see [java-dependencies](../../java-dependencies.md)). This spec is only the store.

Four slices, one PR each, behind the switch below.

1. **Store and create/read.** A document set `observability/objects/` under the tenant's base (a tenant is a deployment base here, as for ISM and LTR): the stored-scripts shape, a marker register naming immutable content blobs. Slice 1 stores each object whole; summaries in the marker are a format change that slice 2 decides. `POST /_plugins/_observability/object` (body `{"<objectType>": {...}}`, answers `{"objectId": "..."}`) and `GET /_plugins/_observability/object/{id}`. Ids are generated (a 22-character URL-safe id) or supplied by `object_id` on the way the plugin allows.
2. **Update, delete, list.** `PUT` and `DELETE /object/{id}`, `DELETE /object?objectIdList=a,b,c` (at most 100), `GET /object` with `objectType`, `sortField` (`lastUpdatedTimeMs`, `createdTimeMs`, `name`, `objectType`), `sortOrder`, `fromIndex`, `maxItems` (at most 100) and `name`. The list reads the names from the marker and one content blob per object it filters, sorts or returns, in pages of at most 100; whether the marker should carry summaries to avoid that is slice 2's decision, with its own format ADR.
3. **Object types.** The plugin's types, each validated by a typed shape and size bound: `savedQuery`, `savedVisualization`, `operationalPanel` (the panel's visualisation ids must exist or are refused 400), `savedNotebook`, `timestamp`, `integration` (stored only; installing an integration bundle is not served), `observabilityDashboard`. An unknown type is 400 `invalid_object_type` naming the supported set.
4. **Stats.** `GET /_plugins/_observability/_local/stats` for this node: request, success and failure counts per route, as the plugin's metrics API does.

Not here: running PPL ([SQL and PPL](../../java-dependencies.md) row), notebook execution, PDF export of a notebook (a [reports](../reports/SPEC.md) concern), integrations that create index templates and dashboards, the Dashboards saved-objects API, and any multi-tenancy that is not this build's tenant. The plugin's `.opensearch-observability` system index is not created and is not a way in or out.

## Acceptance criteria

- AC-1: create, read, replace, delete of each object type through the routes returns the shapes in `shapes.md` beside this spec (taken from the plugin's published API documentation, so `provisional` until a stock recording exists, as LTR's); a missing id is 404 `resource_not_found_exception`, and a stale concurrent replace loses the swap and is 409, never a lost update.
- AC-2: an object's content is at most 1 MiB, a name at most 256 bytes, an object over 1 MiB is refused 400 `illegal_argument_exception` as every other document set refuses one (not 413), a deployment holds at most 1,000 objects (429 `object_limit` beyond), and a body that does not parse or does not match its type's shape is 400 with the failing key, never a 500 or a panic.
- AC-3: list, sort, filter by type and name and page return the objects in order; `maxItems` above 100 is clamped to 100 with the response saying so; an unknown `sortField` is 400.
- AC-4: `DELETE ...?objectIdList=` removes all named objects in one marker swap, reports each id's result, and a list past 100 is 400.
- AC-5: **tenancy**: an object created under one tenant is not readable, listable or deletable under another (404), asserted with two bases on one store; `access` and `tenant` fields in a body are ignored on write and set by the node.
- AC-6: the request meter: create 1 GET + 2 PUT, read 2 GET, replace 1 GET + 2 PUT, delete 1 GET + 1 PUT, list 1 GET + 1 GET per returned object, none lists the store, and an idle node makes zero requests; a row for each lands in [request-budgets](../../request-budgets.md) and `check-budgets.py` holds it.
- AC-7: memory is reserved from the request pool before a body or a marker is held (marker at most 512 KiB: 1,000 summaries at about 400 bytes), asserted by the counting allocator on a replace of a 1 MiB object.
- AC-8: with the switch off every route answers as today (400 `no handler found`, or the proxy when an upstream is configured) ; with it on the existing suite passes unchanged and both run in CI; the prefix is not in the proxy list today, so there is nothing to remove.
- AC-9: a document-set directory written by this build is named in the blob-format listing and the format's round-trip test, and the janitor's orphan sweep does not collect a live content blob.

## Test plan

Slice 1: HTTP tests in `crates/node/tests/server/observability_routes.rs` over the in-process store; red first by an empty route table (404 `no handler found`), then by a store that never swaps (AC-1's stale replace). AC-2 and AC-3 by tables of bodies. AC-5 by two tenant headers on one store; the bug it catches is a marker shared across tenants. AC-6 with the meter, one test per route. AC-9 extends the existing document-set round-trip test and the orphan sweep test with a populated `observability/` directory.

Compatibility: shapes come from the plugin's published documentation and the Dashboards Observability app's requests, not its source (outside this sandbox). A recording from a stock run replaces `shapes.md`; until then AC-1 is `provisional` in VERIFIED.md.

## S3 request budget

The stored-scripts shape, per route as AC-6. The standing cost is zero: no timer, no poll, no refresh. A node caches a marker for `serverless.observability.cache_ttl` (default 30 s, as LTR's) so a Dashboards page that lists and reads many objects makes 0 requests inside the window; the first read after it is the cold cost above. Consistency: a replace through another node is seen after at most the TTL (AC-1 asserts it with a mocked clock).

## Switch

`serverless.observability.enabled` (`OSS_OBSERVABILITY`), default **off** until AC-1 to AC-9 pass; then **on**, because with no schedule, no standing request and a bounded marker it has no measured downside and the proxy prefix stops being needed. The janitor reclaims a content blob left dead by a replace or delete (AC-9 tests that too). Bounds `serverless.observability.max_objects` (1,000) and `max_object_bytes` (1 MiB) are settings.

## Risks

- A marker of 1,000 summaries rewritten per replace is O(objects) bytes per write; at 1,000 objects it is under 512 KiB, which is the bound and why the cap is 1,000. A tenant that wants more is a signal for sharding the marker, not for raising the cap.
- Reproduced shapes may drift from the plugin's as Dashboards versions move; the `shapes.md` recording and a fixture replay in `crates/compat` are the detector.
- Panels refer to visualisations by id: a deleted visualisation leaves a dangling panel entry, as the plugin does; AC-1 pins that a read of such a panel still succeeds.
