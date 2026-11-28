# 0090: A refused single-document write names its root cause as stock does

**Status:** Accepted after independent review, 2026-11-28

## Context

`PUT /<index>/_doc/<id>` answers a refusal as `{"error":{"root_cause":[..],"type":..,"reason":..,"caused_by":..},"status":N}`. The node's single-document answer wrote `type`, `reason` and `caused_by` but no `root_cause`, so a client that reads `error.root_cause[0]` (the stock clients do) found nothing. The recorded stock answers for refused `geo_point` values (`fixtures/compat/geo/point-values.json`) show it on every refusal. The same file lives in the writer's bulk response code, which `scripts/risk-paths.txt` lists as high-risk for what else it holds; this change touches only how a failure is rendered, not what is written or fenced.

## Decision

`write_doc` writes `root_cause` as `OpenSearchException#guessRootCauses` does: the innermost exception of the chain, stopping before a JDK or library exception (a fixed list of types, or one marked plain, as `java.text.ParseException` is though it renders as `parse_exception`). The top-level error renderer uses the same rule, so one function answers both. A bulk item is unchanged: stock writes no `root_cause` there.

## Consequences

- Easier: a client sees the same `root_cause` stock gives; one root rule for both renderers.
- Harder: a new Java exception type that is not an OpenSearch exception needs adding to the list when a fixture shows it.
- A refused `geo_point` carries the cause chain stock reports (the geohash's `IllegalArgumentException`, the WKT tokenizer's parse errors, a number-format error), so the same fixture's `root_cause` agrees; the document crate's refusal chains change with it.
- No S3 request, blob, register or fence changes; error paths only, one small allocation each.
