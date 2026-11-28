# oss-replication

Cross-cluster replication: a follower copies the segment blobs a leader's object store has published ([ADR 0085](../../docs/adr/0085-cross-cluster-replication-copies-published-segments-between-object-stores.md)).

## Owns
Slice 1 ([spec](../../docs/specs/replication/SPEC.md)): the remote settings (`cluster.remote.<alias>.store`), their checks, and `remote::RemoteStore`, a read-only handle on a leader's store that reads a shard's commit manifest and an index's descriptor. No copying, no routes, no poll yet.

## Depends on
`oss-blob` (the store, the read-only wrapper, the root allow-list), `oss-format` (manifest and descriptor parsing, keys). Never `oss-reader`, `oss-writer` or `oss-janitor`.

## Entry points
`settings::remotes` (parse and check a settings update), `settings::Remote::check_roots`, `remote::RemoteStore::{shard_manifest, index_descriptor}`.
