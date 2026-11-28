# Alerting, native: monitors, triggers, destinations, alerts

## Serves

R15.4's alerting, under [ADR 0075](../../adr/0075-alerting-is-ported-natively.md), which supersedes [ADR 0046](../../adr/0046-plugin-requests-are-proxied-to-java-nodes.md) for alerting. The owner's decision (2026-11-28): the stock plugins are ported to Rust, not proxied. A client of the stock Alerting plugin (`opensearch-project/alerting`) must work unchanged against a Rust node.

## Change

Six slices, one PR each, each behind the setting below.

1. **Store.** `oss-meta::alerting`: monitors, destinations and channels, alerts, each a document set (a marker register naming immutable content blobs, the stored-scripts shape) under `alerting/<tenant>/monitors/`, `.../channels/`, `.../alerts/`. Nothing lists. Bounds: a monitor 1 MiB, 1,000 monitors a tenant, 100 triggers a monitor, 100 actions a trigger.
2. **Monitor and destination routes** of the stock plugin, on `/_plugins/_alerting` and `/_opendistro/_alerting`: `POST|PUT|GET|DELETE /monitors[/{id}]`, `POST /monitors/_search`, `POST /monitors/{id}/_execute` (with `dryrun`), `POST /monitors/_execute`, `GET /monitors/alerts`, `POST /monitors/{id}/_acknowledge/alerts`, `GET /alerts`, `GET /destinations` and `/destinations/email_accounts|email_groups` (kept for the legacy shape), `GET /_stats`. Notification channels are the Notifications plugin's `/_plugins/_notifications/configs`; this slice takes channel ids as stored documents and serves the configs routes needed to create the webhook, Slack, Chime and custom-webhook kinds. SNS and email are refused 400 with a message saying so, not silently accepted. Monitor `_version`, `_seq_no` and `_primary_term` follow the set's change count; the plugin's optimistic `if_seq_no` and `if_primary_term` map onto the marker's version.
3. **Query-level monitors.** `monitor_type: query_level_monitor`: the `inputs[0].search` (indices, a query, aggregations) is run through the reader as a search with `size` as the monitor says, the trigger's `condition.script` is evaluated over `ctx` (`ctx.results[0]`, `ctx.monitor`, `ctx.trigger`, `ctx.periodStart`, `ctx.periodEnd`) by the reader's painless subset; a script outside that subset is refused 400 at put, not at run. `{{period_end}}` and `{{period_start}}` are rendered into the query with the Mustache renderer search templates use.
4. **Bucket-level monitors** (`bucket_level_monitor`, composite aggregation `after_key` paging, `buckets_path` and `parent_bucket_path` selectors) **and document-level monitors** (`doc_level_monitor`, queries over a document set). The document-level match uses [`lucene-monitor`](https://github.com/huyz0/lucene-rust/tree/main/crates/lucene-monitor) (the percolator already in lucene-rust: a presearcher narrows the registered queries before a batch is matched), not a per-document search. See "Reuse from lucene-rust" below.
5. **The runner.** A node loads the monitors it is assigned (the placement hash over the monitor id) and computes each one's next slot from its schedule (`period` interval and unit, or `cron` with timezone) and the clock. It sleeps until the earliest slot; no poll. A run is claimed by a conditional create of `alerting/runs/<monitor>/<slot>`; the node that wins runs it, and a loser skips. The run records its alert state, a monitor `last_update_time` and `enabled_time` as the plugin does, and starts no more than `serverless.alerting.max_concurrent_runs` (default 4) at once, with a bounded queue past which a slot is skipped and counted.
6. **Alerts and actions.** Alert states `ACTIVE`, `ACKNOWLEDGED`, `COMPLETED`, `ERROR`, `DELETED`; an alert is created when a trigger first fires, rises with `acknowledge`, completes when the condition clears; throttle windows and `action_execution_policy` (`per_alert`, `per_execution`). An action renders its `message_template` and posts to the channel's URL with a body bound of 1 MiB, a timeout of 10 s, no redirect and no private-address target unless `serverless.alerting.allow_private_targets` is set (a webhook is an SSRF vector). A failed action is recorded in the alert, not retried by a timer.

## Reuse from lucene-rust

lucene-rust has `lucene-monitor` (the Lucene Monitor percolator: query registration, presearcher, batch matching; about 4,600 lines in `src/`), and nothing else alerting-shaped; a code search for "alerting" in that repository finds no hits. It is the matching engine of document-level monitors only. Monitors, triggers, scheduling, alerts and actions are the plugin's behaviour and are built here. Slice 4 depends on `lucene-monitor` at the same pinned revision as the other lucene crates; whether its query analyser covers the DSL queries a document-level monitor takes (`query_string` and `match` against a mapped field) is checked by a test first, and a query it cannot analyse falls back to matching by a plain query over a one-document in-memory reader, bounded by the document batch.

Not here: SNS and email channels; painless beyond the reader's subset; cross-cluster monitors (`cluster_metrics` input beyond `_cluster/health`, `_cluster/stats`, `_nodes/stats` of this node); Java-shell interop; the plugin's `.opendistro-alerting-*` system indices (no index exists; `GET` of them is a 404).

## Acceptance criteria

- AC-1: a monitor is created, read, replaced, searched and deleted through the routes in the plugin's published shapes (`_id`, `_version`, `monitor` with `schedule`, `inputs`, `triggers`); a stale `if_seq_no` is 409; an invalid one is 400 `illegal_argument_exception`; a missing one 404.
- AC-2: put, read and delete are 1 GET and 2 PUTs, 2 GETs, 1 GET and 2 PUTs, no LIST; a search reads the marker and one GET per object of its page of at most 100.
- AC-3: a query-level monitor's `_execute` returns `input_results.results[0]` equal to the same search run directly, and `trigger_results` carrying the condition's boolean; `dryrun` stores nothing (asserted by the request meter).
- AC-4: a condition script outside the supported subset is 400 at put; a script that fails at run gives an `ERROR` alert with the message, never a panic.
- AC-5: a bucket-level monitor pages a composite aggregation to its end within a bound of 10,000 buckets and fires `trigger_results.agg_result_buckets` for the matching buckets only.
- AC-6: a document-level monitor matches each document of a batch against its queries and reports the matching query ids per document, equal to running each query alone, on a fixed fixture.
- AC-7: with a mocked clock, a monitor on `period: 1 MINUTES` runs once per slot across three nodes (asserted by the run claims: one conditional create wins a slot, the others skip), a node that dies is replaced at the next slot by whichever node's create wins, and a slot is never run twice.
- AC-8: **no poll**: with N enabled monitors and a clock advanced one hour with none due, the request meter reads 0; with the clock advanced to a slot, the requests are exactly the claim PUT, the monitor's search and its alert writes. An idle tenant (no monitors) makes no request.
- AC-9: an alert moves `ACTIVE` to `ACKNOWLEDGED` on acknowledge and to `COMPLETED` when the trigger clears, and is not re-created while active; a throttled action does not post twice inside its window.
- AC-10: an action posts the rendered message to a webhook with the bounds above; a redirect and a private address are refused and recorded as `ERROR` in the alert; a slow target times out and the run completes.
- AC-11: memory is reserved before a monitor is held (4 x its bytes from the open-indexes pool) and before a run's search hits and a notification body are held; past the pool a run is skipped and counted, not failed with a 500.
- AC-12: a schedule interval below `serverless.alerting.min_interval` (default 60 s) is 400 at put; a tenant over its monitor count is 429.
- AC-13: with the setting off every route answers as today (400 `no handler found` or the proxy when an upstream is set), and no run starts.

## Test plan

Slice 1 and 2: `crates/meta` unit tests and `crates/node/tests/server/alerting_routes.rs` through HTTP with the request meter for AC-2. Slice 3: `crates/node/tests/it/alerting_query.rs` on a small index. Slice 4: reader unit tests with the document fixture of AC-6. Slice 5: `crates/node/tests/it/alerting_runner.rs` over three emulated nodes with the mocked clock of `clock.rs`, the criteria of AC-7 and AC-8. Slice 6: a local HTTP target in the test for AC-10. Each test fails first (a stub returning no runs, no alert, no match).

Compatibility with stock: no stock run is available yet. Shapes come from the plugin's published API documentation and are provisional (as `ltr/shapes.md`). A recording of a stock Alerting plugin run (the Java shell runs the stock plugin, so the existing `crates/node/tests/mixed_fleet/live_plugins.rs` run is the place) replaces them: each route above gets one recorded request and response, and a shape test compares the Rust answer to it. This is slice 2's last task and gates the default.

## S3 request budget

Management: the stored-scripts shape (a row in [request-budgets](../../request-budgets.md) per route when the routes land). Run: 1 conditional-create PUT to claim a slot, the monitor's search (the reader's cost, charged to the monitor's indices), and 1 to 2 PUTs for the alert document and its marker when state changes; a run whose trigger stays clear and that has no alert makes no write beyond the claim. Load: a node reads a tenant's monitor marker once per `serverless.alerting.refresh_ttl` (default 60 s) **only while that tenant has an enabled monitor on this node**, so the steady bill is (tenants with monitors) x (nodes holding them) / TTL, not per index or per elapsed time per idle index; a monitor-less tenant makes none. Depth: one round for the claim, then the search's own rounds; fan-out free. LIST: none.

The claim PUT per run is a bill that scales with monitors x runs, which is the feature's nature (the stock plugin writes the same run state to its system index); the interval floor and the monitor count bound cap it, and AC-8 asserts nothing is paid between slots.

## Switch

`serverless.alerting.enabled` (`OSS_ALERTING`), default off; with it off the alerting prefixes go to a plugin upstream when one is configured, and the proxy's prefixes for alerting are removed when the default turns on, not before. It flips on only when VERIFIED.md records: the stock recording of slice 2, the request counts of AC-2, AC-7 and AC-8, a run's search latency against the same search issued by a client, and the memory of 1,000 held monitors. Also `serverless.alerting.min_interval`, `max_monitors`, `max_concurrent_runs`, `refresh_ttl` and `allow_private_targets`.

## Risks

- Reference behaviour is from the plugin's documentation, not its source (`opensearch-project/alerting` is outside this sandbox's repositories); shapes are provisional until a stock recording exists.
- A scheduler without a leader depends on the claim create being atomic in every backend; the emulator and RustFS are checked by the conformance suite, the fs store by its single host.
- Clock skew between nodes can make two nodes believe a slot is theirs at different instants; the claim decides, so the cost is a skipped run in the worst case, never a double run. A slot a node was too late for is not backfilled.
- A noisy tenant with many frequent monitors is a cost problem more than a correctness one: the interval floor, the monitor bound and the per-tenant meter are the guard, and the measurement for the default decides whether they are tight enough.
- `lucene-monitor`'s analyser may not cover every DSL query; the fallback is bounded but slower.
