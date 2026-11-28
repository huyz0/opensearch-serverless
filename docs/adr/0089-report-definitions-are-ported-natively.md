# 0089: Report definitions are ported natively over a document set; rendering stays out of the node

**Status:** Accepted, 2026-11-28, under the ADR rule: the decision applies the store shape of [ADR 0088](0088-observability-is-ported-natively.md) to a new family and keeps the browser out of the node, which the independent review of the [reports spec](../specs/reports/SPEC.md) found keeps the S3 request cost, bounded memory and scalability rules.

## Context

The stock reporting plugin stores report definitions and instances; the Dashboards reporting app draws a PDF or PNG with a headless browser. The owner's rule (2026-11-28) is that every plugin is native with no Java in the runtime path.

## Decision

Definitions are documents of a new document-set kind, `DocKind::ReportDefinitions`, container `reports/definitions/` under the deployment's base: a marker register naming immutable content blobs, written by compare-and-swap, never listed. This adds one directory name to the shared blob formats and changes no existing byte; a Java node ignores it. The kind holds at most 500 documents (`DocKind::max_docs`, where every other kind holds 1,000) and 1 MiB each. The routes sit behind `serverless.reports.enabled` (`OSS_REPORTS`), default on. A PDF or PNG definition is refused 501 `renderer_not_configured` until the renderer tier of the spec exists; a delivery to notification channels is refused 501 `delivery_unsupported` until the delivery slice.

## Consequences

- Easier: the definition API answers without a Java node; a create is 1 GET and 2 PUT, a read 2 GET, idle makes none.
- Harder: a list reads one content blob per definition, at most 500, because a marker holds names, not summaries.
- Rules out: Java nodes reading the Rust store, and a browser inside a node.
