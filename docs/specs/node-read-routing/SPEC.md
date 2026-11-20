# Cluster-wide document get and refresh

## Serves

R7: `GET|HEAD /{index}/_doc/{id}`, `/{index}/_source/{id}` and `/{index}/_refresh` answer for every shard of the index, not only the ones this node holds ([read forward](../node-read-forward/SPEC.md), [doc read routes](../node-doc-read-routes/SPEC.md), [cluster routing](../node-cluster-routing/SPEC.md)).

## Change

- `Routes` gains `get_remote` (default: an error). `ClusterRoutes::with_reads` gives it a `GetSend`, and it asks the owner for the document, answering the document or `None`.
- `WriterRole::get_owned` and `refresh_owned` read or refresh on this node when it holds the shard and otherwise ask the owner (a refresh is the existing empty bulk with the refresh flag).
- The get and `_refresh` routes use them. A get whose owner cannot be asked answers 503 `no_shard_available_action_exception` naming the shard, the owner and the reason; a refresh counts that shard failed.
- `boot` registers the get handler beside the bulk one and builds the forwarder over the same transport client and leases.

## Acceptance criteria

- AC-1: a get or source get of a shard another node owns answers what the owner returns (version, sequence number, term, source) with the same body as a local one; a missing id there is 404.
- AC-2: a get whose owner cannot be asked answers 503 `no_shard_available_action_exception` naming the owner and the reason.
- AC-3: a refresh of an index counts a remote shard successful when its owner refreshed it, and failed when it could not be asked.
- AC-4: `ClusterRoutes::get_remote` sends one `GetRequest` with the index, its uuid, the shard and the id to the owner, maps a found answer to the document and a not-found one to `None`, and fails when the call fails or no sender is configured.
- AC-5: two booted nodes: every document written through one is found through either, and a refresh through one leaves no shard failed.

## Test plan

`crates/node/tests/get_route.rs` (AC-1 to AC-3, with a test `Routes`), `tests/cluster_owner.rs` (AC-4), `tests/boot.rs` (AC-5, real nodes, real transport, signed).

## S3 request budget

No blob request: the owner comes from the leases already held; one transport call per forwarded get or refreshed shard.

## Risks

- A forwarded get is always realtime; `realtime=false` on a remote shard still sees unpublished writes.
- A refresh asks each remote shard in turn, one call a shard; an index of many remote shards makes that many sequential calls.
