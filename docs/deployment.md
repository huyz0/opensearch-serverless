# Deployment: roles and ingress

A node serves the roles `OSS_ROLES` names. Every node runs the same binary and coordinates any request it receives; the roles say what else it does.

| `OSS_ROLES` | The node |
|---|---|
| unset, empty, or `ingest,search` | Takes writes and serves searches (the default; one pool is enough to start). |
| `ingest` | Takes writes and owns shards: opens them, takes a dead node's, publishes. Writers do not push their recent writes to it. |
| `search` | Serves searches and reads from published segments, and receives the writers' pushes of recent writes. Owns no shard and takes none. |
| `none` | Coordinates only: it routes each request and holds nothing. |

## What a node without a role does

- **No `ingest`:** it opens no shard, takes no dead node's shards, and never writes a log. A write that reaches it goes to the owner among the live `ingest` nodes (the same rendezvous every node computes over the same leases). With no `ingest` node alive, the write fails 503 `unavailable_shards_exception` and no shard is opened.
- **No `search`:** writers never push to it. It still answers a search sent to it, from published segments, so a search through it may not see a write the `search` pool already holds in its tail overlay. Send searches to the `search` pool.

Roles are read at start; changing them takes a restart. A node's lease names them, so peers see a change within a lease refresh.

## Two pools behind one ingress

Run two groups of nodes with the same `OSS_CLUSTER`, store and base, one with `OSS_ROLES=ingest` and one with `OSS_ROLES=search`, behind one ingress that splits by path and method:

| Requests | Pool |
|---|---|
| `POST /_bulk`, `POST /{index}/_bulk`, `PUT`, `POST` and `DELETE` on `/{index}/_doc/*`, `/{index}/_create/*`, `/{index}/_update/*`, `POST /{index}/_delete_by_query` | `ingest` |
| `GET` and `POST` on `/_search`, `/{index}/_search`, `/_msearch`, `/{index}/_count`, `/_pit`, `GET /{index}/_doc/*` | `search` |
| Index and cluster administration, `_cat`, `_cluster`, `_serverless` | either |

No proxy needs to know which node owns a shard: each node forwards to the owner. The split only decides which pool carries the write and search load, so each scales on its own and a search burst does not take write capacity.

An ingress that sends everything to one pool needs no split: with both roles on every node, the node that receives a request routes it.

## Limits

- An `ingest`-only node is not sent the writers' pushes, so a search through it is not measured against the tail overlay; keep searches on the `search` pool.
- Readers are not chosen by role for a search fan-out yet: a search is served by the node it reaches.
