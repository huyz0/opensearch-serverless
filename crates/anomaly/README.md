# oss-anomaly

The anomaly model of the native anomaly detection plugin ([spec](../../docs/specs/anomaly-detection/SPEC.md), [ADR 0078](../../docs/adr/0078-anomaly-detection-is-ported-natively.md)): a Random Cut Forest over shingled feature vectors, a thresholder that turns its score into a grade and a confidence, imputation of missing windows, and a checksummed checkpoint. Pure compute: no I/O, no clock, no threads, no dependencies.

## Owns
The model's state and its transitions: sampling, trees, scoring, thresholding, shingling and the checkpoint bytes.

## Depends on
Nothing in the workspace.

## Entry points
`src/lib.rs` re-exports them:

- `Params` (validated; `reserved_bytes()` is what a node reserves before building a model, `workspace_bytes()` what one worker thread reserves for its scratch).
- `Model::new(params)`, then `feed(&mut workspace, slot, Some(values) | None)` per window, which returns a `Scored { score, grade, confidence }` once the model is ready.
- `Model::checkpoint()` and `Model::restore(bytes)`: a restored model is bit-identical to the one checkpointed, and a corrupt, truncated or future-version blob is an error, never a panic.

Determinism is per build: scoring and sampling use the platform's `ln`, `log2` and `sqrt`, so equal seeds and inputs give equal bytes on one binary, not across libms.
