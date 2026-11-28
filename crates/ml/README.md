# oss-ml

The model-side code of the native ML Commons plugin ([spec](../../docs/specs/ml-commons/SPEC.md), [ADR 0079](../../docs/adr/0079-ml-commons-is-ported-natively.md)). It wraps `lucene-ml` (huyz0/lucene-rust, the pinned revision) so the rest of the workspace does not depend on ONNX Runtime or the tokenizer directly. Today it holds the connector credential cipher; the remote and local model runners come in later slices.

## Owns
The cipher for connector credentials: a master key, `seal` and `open`.

## Depends on
`lucene-ml` only. No workspace crate.

## Entry points
`credentials::new_master_key`, `credentials::seal(master_key, plaintext)` and `credentials::open(master_key, sealed)`; each returns the cipher's error text on failure, never panics on a malformed input.
