# Observability objects, native: saved queries, visualisations, panels and notebooks as stored documents

## Serves

The "Observability" row of [java-dependencies](../../java-dependencies.md) and the owner's rule (2026-11-28) that every plugin is native with no Java in the runtime path. Pattern: [LTR](../ltr/SPEC.md) and [ADR 0071](../../adr/0071-learning-to-rank-is-ported-natively.md). [ADR 0088](../../adr/0088-observability-is-ported-natively.md): the store shape of [ADR 0073](../../adr/0073-stored-painless-scripts-resolve-through-a-ttl-cache.md) applied to a new family, with one document-set directory and no process or timer.

## Change

The stock Observability plugin is, on the server, a store of typed documents behind `/_plugins/_observability/object`; the Dashboards Observability app keeps its saved queries, visualisations, operational panels, notebooks and timestamps there. Nothing runs on a schedule and nothing executes a saved query: the PPL a saved query holds is run by the client through `/_plugins/_ppl`, which is a different row (not served, see [java-dependencies](../../java-dependencies.md)). This spec is only the store.

Four slices, one PR each, behind the switch below.

1. **Store and create/read.** A document set `observability/objects/` under the tenant's base (a tenant is a deployment base here, as for ISM and LTR): the stored-scripts shape, a marker register naming immutable content blobs. Slice 1 stores each object whole; summaries in the marker are a format change that slice 2 decides. `POST /_plugins/_observability/object` (body `{"<objectType>": {...}}`, answers `{"objectId": "..."}`) and `GET /_plugins/_observability/object/{id}`. Ids are generated (a 22-character URL-safe id) or supplied by `objectId` in the body.
2. **Update, delete, list.** `PUT` and `DELETE /object/{id}`, `DELETE /object?objectIdList=a,b,c` (at most 100, all of them or none), `GET /object` with `objectType` (comma list), `objectIdList`, `sortField` (`lastUpdatedTimeMs` the default, `createdTimeMs`, `name`, `objectType`), `sortOrder`, `fromIndex`, `maxItems` (at most 100, the default) and `name` (exact). The list reads the marker and one content blob per object, in one walk, keeping at most 4 MiB of what it read so a page of small objects needs no second read; summaries in the marker would avoid the walk and are a format change with its own ADR, not made here.
3. **Object types and the stock recording.** The plugin's behaviour is the recording `fixtures/compat/observability/observability-objects.json` (stock 3.10.0), replayed by `tests/server/observability_replay.rs`. Types: `savedQuery`, `savedVisualization`, `application`, `operationalPanel`, `notebook`, each kept as the plugin keeps it (an `operationalPanel` keeps its `name` alone; a body naming two types stores the first in that order; `savedQuery: {}` is valid). Anything else is 400 `Object type field absent`. Refusals are the recorded ones: 409 on a duplicate `objectId` or on a type change, 404 for a missing id, 406 for an unknown `sortField`, 400 for a bad `sortOrder`, `fromIndex` or `maxItems`.
4. **Not served**, as the stock plugin does not: `/_plugins/_observability/stats`, `/_plugins/_notebooks/...`, `/_opendistro/_observability/...` answer 400 `no handler found`.

Not here: running PPL ([SQL and PPL](../../java-dependencies.md) row), notebook execution, PDF export of a notebook (a [reports](../reports/SPEC.md) concern), integrations that create index templates and dashboards, the Dashboards saved-objects API, and any multi-tenancy that is not this build's tenant. The plugin's `.opensearch-observability` system index is not created and is not a way in or out.

## Acceptance criteria

- AC-1: create, read, replace and delete of each object type return what the stock recording returns (times and parser positions masked); a missing id is 404 `status_exception` `ObservabilityObject <id> not found`; a replace that changes the type is 409. A concurrent replace of one object is last writer wins in this build (the document set has no conditional replace); the plugin's own optimistic check is not reproduced.
- AC-2: an object's content is at most 1 MiB, a name at most 256 bytes, an object over 1 MiB is refused 400 `illegal_argument_exception` as every other document set refuses one (not 413), a deployment holds at most 1,000 objects (429 `object_limit` beyond), and a body that does not parse or does not match its type's shape is 400 with the failing key, never a 500 or a panic.
- AC-3: list, sort, filter by type and name and page return the objects as recorded; `maxItems` above 100 is clamped to 100; an unknown `sortField` is 406. Known divergence: a `name` sort across mixed types orders by the name alone, where the stock plugin's order is not shown by the recording.
- AC-4: `DELETE ...?objectIdList=` removes all named objects, or none when one is missing (404 naming the missing ones), one marker swap each, and a list past 100 is 400.
- AC-5: **tenancy**: an object created under one tenant is not readable, listable or deletable under another (404), asserted with two bases on one store; `access` and `tenant` fields in a body are ignored on write and set by the node.
- AC-6: the request meter: create 1 GET + 2 PUT, read 2 GET, replace 1 GET + 2 PUT, delete 1 GET + 1 PUT, list 1 GET + 1 GET per object in the deployment (at most 1,000), none lists the store, and an idle node makes zero requests; a row for each lands in [request-budgets](../../request-budgets.md) and `check-budgets.py` holds it.
- AC-7: memory is reserved from the request pool before a body or a marker is held (marker at most 512 KiB: 1,000 summaries at about 400 bytes), asserted by the counting allocator on a replace of a 1 MiB object.
- AC-8: with the switch off every route answers as today (400 `no handler found`, or the proxy when an upstream is configured) ; with it on the existing suite passes unchanged and both run in CI; the prefix is not in the proxy list today, so there is nothing to remove.
- AC-9: a document-set directory written by this build is named in the blob-format listing and the format's round-trip test, and the janitor's orphan sweep does not collect a live content blob.

## Test plan

Slice 1: HTTP tests in `crates/node/tests/server/observability_routes.rs` over the in-process store; red first by an empty route table (404 `no handler found`), then by a store that never swaps (AC-1's stale replace). AC-2 and AC-3 by tables of bodies. AC-5 by two tenant headers on one store; the bug it catches is a marker shared across tenants. AC-6 with the meter, one test per route. AC-9 extends the existing document-set round-trip test and the orphan sweep test with a populated `observability/` directory.

Compatibility: the stock recording above is the contract.

## S3 request budget

The stored-scripts shape, per route as AC-6. The standing cost is zero: no timer, no poll, no refresh. A node caches a marker for `serverless.observability.cache_ttl` (default 30 s, as LTR's) so a Dashboards page that lists and reads many objects makes 0 requests inside the window; the first read after it is the cold cost above. Consistency: a replace through another node is seen after at most the TTL (AC-1 asserts it with a mocked clock).

## Switch

`serverless.observability.enabled` (`OSS_OBSERVABILITY`), default **on** (the owner's rule of 2026-11-28: a ported plugin needs no flag, or its flag is on by default), because with no schedule, no standing request and a bounded marker it has no measured downside and the proxy prefix stops being needed. The janitor reclaims a content blob left dead by a replace or delete (AC-9 tests that too). Bounds `serverless.observability.max_objects` (1,000) and `max_object_bytes` (1 MiB) are settings.

## Risks

- A marker of 1,000 summaries rewritten per replace is O(objects) bytes per write; at 1,000 objects it is under 512 KiB, which is the bound and why the cap is 1,000. A tenant that wants more is a signal for sharding the marker, not for raising the cap.
- Reproduced shapes may drift from the plugin's as Dashboards versions move; the `shapes.md` recording and a fixture replay in `crates/compat` are the detector.
- Panels refer to visualisations by id: a deleted visualisation leaves a dangling panel entry, as the plugin does; AC-1 pins that a read of such a panel still succeeds.
