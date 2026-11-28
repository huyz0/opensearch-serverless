# 0078: Anomaly detection is ported natively; it is no longer proxied

**Status:** Proposed (independent review round 1: accept with changes, folded in; a second round decides)

## Context

[ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md) hands the anomaly detection prefixes to a Java node, and it ran live there (detector CRUD and search, [plugin-proxy VERIFIED](../specs/plugin-proxy/VERIFIED.md)). The owner decided on 2026-11-28, as for learning to rank ([ADR 0071](0071-learning-to-rank-is-ported-natively.md)) and alerting ([ADR 0075](0075-alerting-is-ported-natively.md)), that the stock plugins are ported to Rust. Anomaly detection differs from both: it is **stateful and scheduled**. A started detector runs a search every interval and feeds a Random Cut Forest model that must survive a node loss. The stock plugin does this with the Job Scheduler plugin's timer, a polled system index and checkpoint documents.

Forces: idle indices make zero requests; no lock, lease or leader election for correctness; requests scale with nodes and bytes, never with elapsed time per index; memory is reserved before it is held ([ADR 0004](0004-bounded-memory-and-streaming.md)); lucene-rust holds no anomaly code, and no Rust Random Cut Forest crate exists on crates.io.

## Decision

Anomaly detection is native in `oss-anomaly` (a new leaf crate: the model), `oss-meta` (the store and the shared scheduler), `oss-reader` (the feature searches, through the ordinary search path) and `oss-node` (the routes and the runner), as [the spec](../specs/anomaly-detection/SPEC.md) lays out.

- **The model is written here**, from the Random Cut Forest paper and the library's documented thresholding, as a pure crate with a serialisable state; scores are compared with the stock plugin by flagged windows on committed fixtures, not by value, because the stock library samples randomly.
- **State is blobs under `ad/`**, by compare-and-swap, never listed: detectors, and a checkpoint of each started detector's models as immutable chunks of at most 8 MiB (64 MiB a detector at most) named by one marker whose compare-and-swap is conditioned on the etag read at load, so a paused old owner cannot overwrite a newer checkpoint. Results are ordinary documents of a result index, written through the node's write log, so a run adds no PUT of its own for them.
- **The scheduler is alerting's** ([ADR 0075](0075-alerting-is-ported-natively.md)): a registry of 256 markers read once a TTL, a claim by one compare-and-swap per run, at most once per slot, windows from slot boundaries, no poll per detector. AD's runner slice is blocked until alerting's has merged, then extracts it into one shared module with the registry's 256 shard markers shared, so the rule is written once and AD adds no standing marker reads beyond its own tenants'.
- **One owner, not two.** A detector's owner is the single live node with the highest placement hash, so its model stays in one memory; a node uses its model only if it was the previous claimer and otherwise reloads the checkpoint, so a flap costs a read and never two model lineages. Membership disagreement costs a failed claim, never a double run.
- **A started detector is a standing job, stated as such.** Its cost is claim PUT, search and result write per interval, plus a checkpoint each `checkpoint_interval`; a stopped or deleted detector costs nothing. The 60 s floor, per-tenant counts and a fleet cap bound it; the worst case per tenant is written in the spec.
- **Memory is bounded by reservation:** each model is reserved at construction from a cache pool; past the pool a new entity is skipped and counted, never evicting a model with a checkpoint pending.
- The feature is behind `serverless.ad.enabled`, **off** until the spec's criteria pass, including a recording of the stock plugin.

## Consequences

- Easier: detectors run on a Rust cluster without a Java node; each run is visible in the request meter; a takeover is a checkpoint read.
- Harder: a takeover loses up to one `checkpoint_interval` of model updates; high-cardinality detectors are capped by model memory; plugin-visible shapes are reproduced from documentation until a stock recording exists; a second port shares one scheduler, so the order of the two extraction PRs matters.
- Rules out: Java nodes reading the Rust store; bit-equal scores with the stock plugin; the forecasting plugin and ML Commons' RCF functions (separate ports).
- ADR 0046's proxy keeps ISM and ML Commons until their ports land; its anomaly-detection prefixes leave the list when the default turns on, not before, so the existing proxy tests and the new native tests both run in CI.
