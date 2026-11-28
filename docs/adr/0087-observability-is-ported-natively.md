# 0087: Observability objects are ported natively over a document set; they are no longer left to a Java node

**Status:** Accepted, 2026-11-28, under the ADR rule: the decision is the store shape of [ADR 0073](0073-stored-painless-scripts-resolve-through-a-ttl-cache.md) and [ADR 0071](0071-learning-to-rank-is-ported-natively.md) applied to a new family, which the independent review of the [observability spec](../specs/observability/SPEC.md) found keeps the S3 request cost, bounded memory and scalability rules.

## Context

The observability plugin's server side is a store of typed JSON objects (saved queries, visualisations, panels, notebooks) behind `/_plugins/_observability/object`. The owner's rule (2026-11-28) is that every plugin is native with no Java in the runtime path. Nothing runs on a schedule and nothing executes a stored query, so the port is a store.

## Decision

Objects are documents of a new document-set kind, `DocKind::ObservabilityObjects`, container `observability/objects/` under the deployment's base: a marker register naming immutable content blobs, written by compare-and-swap, never listed. This adds one directory name to the shared blob formats and changes no existing byte; a Java node ignores it, and no Java node reads it. The bounds are the kind's own: at most 1,000 objects a deployment and 1 MiB each, the document set's common bounds. The routes sit behind `serverless.observability.enabled` (`OSS_OBSERVABILITY`), default off until the spec's criteria pass.

## Consequences

- Easier: the plugin's object API answers without a Java node; a create is 1 GET and 2 PUT, a read 2 GET, idle makes none.
- Harder: a list (slice 2) reads one content blob per returned object, because a document set's marker holds names, not summaries; the spec's marker-summary design needs a marker format change, which waits for its own decision.
- Rules out: Java nodes reading the Rust store, and the plugin's `.opensearch-observability` index.
