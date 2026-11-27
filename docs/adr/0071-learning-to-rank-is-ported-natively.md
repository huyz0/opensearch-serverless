# 0071: Learning to rank is ported natively to the reader; it is no longer proxied

**Status:** Accepted (independent review, two rounds, 2026-11-27)

## Context

[ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md) lists learning to rank (LTR) among the plugins whose requests a Rust node hands to a Java node. The proxy covers a request whose path begins with a plugin prefix. LTR's use is the `sltr` query inside an ordinary `_search` rescore and a feature-logging section of the search body, and those go to the native `_search` route, not to a prefix: behind the proxy a Rust node would offer the store's management routes and refuse the query the store exists for. The owner decided on 2026-11-27 that LTR is ported to Rust, not run as is.

Forces: requests scale with nodes and bytes ([ADR 0004](0004-bounded-memory-and-streaming.md), principles); a model is data (tree ensembles, weights), not code, so it needs no sandbox; the reader already runs a rescore stage ([search-rescore](../specs/search-rescore/SPEC.md)) and the Mustache renderer of search templates.

## Decision

LTR is native in `oss-reader` (evaluators, the `sltr` query, `ltr_log`) and `oss-node` (the routes), as [the LTR spec](../specs/ltr/SPEC.md) lays out. The store is blobs under `ltr/<store>/`, written by compare-and-swap, never listed, read through a per-node cache keyed by `(store, name, version)` whose freshness is a TTL checked lazily by a search (stale answer served, one background GET of the store's marker, no timer and no poll), so idle makes no request and the steady bill is bounded by stores used times nodes over the TTL, not by search rate. Models are evaluated by native evaluators for linear, XGBoost JSON gbtree and RankLib ensembles; no extension point is added (a custom ranker would be a `scoring` point, which this does not need). The `/_plugins/_ltr` and `/_ltr` prefixes leave the proxy's list when the native routes land. The feature is behind `serverless.ltr.enabled`, off until the spec's criteria pass.

## Consequences

- Easier: a Rust cluster ranks with LTR without a Java node; no per-query hop.
- Harder: plugin-visible shapes are reproduced from documentation and reference-tool output, not from the plugin's source, which this sandbox cannot read; they stay provisional until a stock recording exists.
- Rules out: Java nodes reading the Rust store (the plugin's `.ltrstore` index and the Rust blobs are separate), and RankLib rankers beyond the tree ensembles.
- ADR 0046's proxy stays for ISM, alerting, anomaly detection and ML Commons.
