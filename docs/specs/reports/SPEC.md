# Reports, native: definitions, instances, on-demand and scheduled; rendering as an out-of-process tier

## Serves

The "Reports (Dashboards reporting)" row of [java-dependencies](../../java-dependencies.md). Owner rule (2026-11-28): native, no Java in the runtime path, native plugin tier per [ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md). Depends on [alerting](../alerting/SPEC.md)'s scheduler (slice 5, `oss-meta::schedule`) and on a Notifications port that does not exist yet.

## Change

The stock reporting backend (`opensearch-project/reporting`) stores **report definitions** and **report instances** and, for a scheduled definition, creates an instance on its schedule with the Job Scheduler. It does not draw anything: a PDF or PNG of a dashboard is made by the Dashboards reporting app with a headless browser, a CSV or XLSX of a saved search by a search export. This build keeps that split and decides who renders:

- **CSV and XLSX of a saved search** are rendered natively in the node: a `_search` the reader runs, streamed into a bounded writer (no whole-body read). The saved search is an [observability](../observability/SPEC.md) or Dashboards saved object resolved by id.
- **PDF and PNG of a dashboard, visualisation or notebook** need a browser. The node never starts one. The renderer is a **`grpc`-tier service** under ADR 0080 (`render` point, effectful, a bounded request: the page URL, the format, the window size, and a short-lived read token the node mints for the tenant; the answer is the bytes within a size bound), run as a sidecar by the operator like any Envoy external service. With none configured these formats are refused 501 `renderer_not_configured` at definition create, not at run time.

Five slices, one PR each, behind the switch below.

1. **Definitions.** `POST /_plugins/_reports/definition`, `GET` (list, paged) and `GET|PUT|DELETE /definition/{id}`, in the stored-scripts document-set shape under `reports/<tenant>/definitions/`. A definition is validated against the stock shape: `report_params` (`report_name`, `report_source` of Dashboard, Visualization, Saved search or Notebook, `core_params` with `base_url`, `report_format`, `time_duration`, `origin`), `delivery` (Notifications `configIds`, `title`, `textDescription`, `htmlDescription`) and `trigger` (`trigger_type` Download, On demand or Schedule; for Schedule a `trigger_params` of a recurring `interval` or a `cron` with a time zone, `enabled`).
2. **Instances and on-demand.** `POST /_plugins/_reports/on_demand` (inline definition) and `POST /definition/{id}/on_demand`-equivalent create an instance; `GET /instance`, `GET /instance/{id}` list and read; `PUT /instance/{id}` updates a status; `GET /stats`. An instance is `Success`, `Error` or `Shared` with a state description and the generated file's reference. The CSV and XLSX render happens in the request or a [task](../../adr/0072-tasks-are-node-local-memory-with-no-blob-requests.md) and its result is written to one content blob through `oss-blob`'s streamed (multipart) write, bounded.
3. **Schedules.** A Schedule definition that is enabled makes an instance each slot, claimed by the compare-and-swap of the scheduler (at most once per slot, a node that dies after the claim loses that slot, as alerting). No timer polls an index; a node sleeps until the earliest due slot.
4. **Delivery.** After an instance succeeds, each `configId` is a Notifications channel; the send goes through the Notifications port. Until that port exists the delivery is refused 501 `delivery_unsupported` at definition create, not skipped.
5. **PDF and PNG** through the `render` point and the gRPC renderer, with a conformance fixture renderer in the test tree (not a browser) so the host's contract, limits and failures are tested without one.

Not here: the Dashboards reporting app's client side, the report "menu" and "scheduled report" UI views (they use these routes), reports of external URLs (refused: `base_url` must be an origin the policy lists), and email beyond what Notifications gives.

## Acceptance criteria

- AC-1: definition create, read, replace, list and delete return the shapes in `shapes.md` (`provisional` until a stock recording exists); an unknown `report_source` or `report_format` is 400, a malformed cron or an interval under `serverless.reports.min_interval` (default 300 s) is 400, a missing id 404.
- AC-2: an on-demand CSV of a saved search returns the rows the same search returns through `_search`, in the same order, with the plugin's column order and quoting, for at least a 100,000-hit result with `max_rows` set to 200,000 in the test, with peak memory bounded by the writer's buffer (counting allocator) and not by the hit count.
- AC-3: a CSV past `serverless.reports.max_rows` (default 10,000, as the plugin's) is truncated with `state_description` saying so; an instance's stored file is at most `max_file_bytes` (default 50 MiB), past which the instance is `Error`, never a partial success.
- AC-4: an enabled Schedule definition makes exactly one instance per slot with two nodes racing (asserted with a mocked clock and two schedulers on one store), makes none for a slot whose end is not yet past by the skew allowance, and makes none for a disabled definition.
- AC-5: **no poll**: with 1,000 enabled definitions and the clock advanced one hour with none due, the request meter reads exactly the scheduler's registry reads and nothing per definition or instance.
- AC-6: a definition for PDF or PNG with no renderer configured is 501 `renderer_not_configured`; with the fixture renderer the host sends the bounded request, validates the answer (size, content type magic), kills a call at its deadline, opens the breaker after the plugin system's failure count, and never forwards the caller's credentials, only the minted read token.
- AC-7: the minted token reads only the report's tenant, expires at the call's deadline, and is refused on every other route (asserted against the security action table).
- AC-8: a tenant holds at most 500 definitions and 1,000 instances (a create beyond is 429 `report_limit`; instances are removed by DELETE or by `serverless.reports.instance_ttl`, applied when the tenant next writes an instance, never by a timer); fleet-wide `serverless.reports.max_total_definitions` (default 100,000) and `max_concurrent_runs` per node (default 4, a run past it waits for the next slot and is counted) are held by a test each.
- AC-9: with the switch off every route answers as today and the proxy list has no `_reports` entry to remove (it is not served today); with it on the existing suite passes unchanged and both run in CI.
- AC-10: the request meter: definition create 1 GET + 2 PUT, read 2 GET; an instance's create 1 GET + 3 PUT (marker, record, file) and a scheduled slot adds the claim PUT only; none lists; rows land in [request-budgets](../../request-budgets.md).

## Test plan

Slice 1 as the observability spec (HTTP over the in-process store, shapes tables). Slice 2: `crates/node/tests/server/reports_csv.rs` with a generated index and a counting allocator for AC-2/AC-3; red first by a writer that collects all rows. Slice 3: two schedulers sharing one store and a mocked clock, the bug caught is a double run. AC-6 and AC-7 against a fixture renderer in `crates/ext` tests. Every refusal (501s) is tested with the switch on.

## S3 request budget

Management: the stored-scripts shape. A scheduled run: the claim (1 PUT), the search (the reader's cost, charged to the tenant), the file (1 PUT) and the instance record (1 swap). Standing cost: the scheduler's registry reads (alerting's budget), which this spec does not add to. Worst case per tenant: 500 definitions at the 300 s floor is 144,000 runs a day, each a search; the floor, the count and the per-tenant meter are the guard.

## Switch

`serverless.reports.enabled` (`OSS_REPORTS`), default **off**: scheduled reports spend searches by design, as monitors do. It flips on when slices 1 to 3 pass and a one-day soak of 1,000 definitions shows the idle bill equals the scheduler's. `serverless.reports.renderer` (a gRPC address, empty by default) turns on slice 5.

## Risks

- The plugin's shapes are documented, not read from source; slice 1 records them as provisional.
- Rendering through a browser sidecar is the operator's to run and secure; the host's limits (size, time, token) bound the damage but the sidecar fetches a Dashboards URL, so `base_url` is allow-listed and a private address is refused as in alerting's egress rule.
- The scheduler and Notifications are not built. Slices 3 and 4 are blocked on them and stay blocked rather than ship a private timer.
