# 0078: ML Commons is ported natively to Rust; it is no longer proxied

**Status:** Proposed (independent review round 1 folded in; awaiting round 2)

## Context

[ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md) lists ML Commons among the plugins a Rust node hands to a Java node. The owner decided on 2026-11-28 that the remaining plugins are ported to Rust as learning to rank was ([ADR 0071](0071-learning-to-rank-is-ported-natively.md)). lucene-rust already holds `lucene-ml` (ONNX and TorchScript inference, tokenizer, the embedding, sparse, cross-encoder and highlighting models, the remote connector engine and credential cipher, checked against stock vectors) and a Java plugin layer over `.plugins-ml-*` indices that gives the REST shapes. A Rust-only node has no such indices, and the work to port is the registry, the routes, deploy and predict, and the agents on top.

Forces: requests scale with nodes and bytes ([ADR 0004](0004-bounded-memory-and-streaming.md), principles); a connector call goes to a user-supplied URL; model files are large; credentials must stay encrypted.

## Decision

ML Commons is native in a new crate `oss-ml` (the registry over `DocSet`s under `ml/`, connectors, inference dispatch, bounded outbound HTTP) and `oss-node` (the routes), as [the spec](../specs/ml-commons/SPEC.md) lays out, taking `lucene-ml` as a git dependency at the revision the workspace pins. State is blobs written by compare-and-swap and never listed; tasks are [ADR 0072](0072-tasks-are-node-local-memory-with-no-blob-requests.md)'s registry, unchanged; every fetch of a user-supplied URL goes through one client that refuses redirects, private addresses and anything outside a deny-by-default allow-list, pinned to the address it resolved; model files are streamed blobs charged to the memory pool before they load; a predict reads through a TTL cache so a warm one makes no request. Agents and memory follow as later slices and use the cluster's ordinary indices for conversation data. The feature is behind `serverless.ml.enabled`, off until the spec's criteria pass. The `/_plugins/_ml` prefix leaves the proxy's list when the first routes land.

## Consequences

- Easier: a Rust cluster registers models, runs remote and local inference and, later, agents without a Java node.
- Harder: shapes are reproduced from the reference plugin's Java and its comparison script, not stock's source; they stay provisional until recorded from stock. The binary gains ONNX Runtime and tokenizer dependencies, isolated in `oss-ml`.
- Rules out: Java nodes reading the Rust store; the classic algorithms (`_train` and the like) in the first slices.
- ADR 0046's proxy stays for ISM, alerting and anomaly detection.
