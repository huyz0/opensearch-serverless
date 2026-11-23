# A node assembled from its roles

## Serves

Parity plan R7.6, node assembly: until now each route family was built by hand in its own test. `assemble` is the composition root: given the node's store, memory budget and a writer's committer and routing, it builds every service a node with the reader and writer roles serves, registers all their routes on one router, and starts the publish timer. A node's `main` is configuration plus this call (a later piece).

## Change

`oss-node`, with the `reader` and `writer` features, gains `assemble`: `Config`, `Writer`, `assemble` and `Running`.

- One store: the caller's `Arc<dyn BlobStore>` is wrapped in `oss_blob::Shared`, so the search service, the writer's publishers and the point-in-time store reach the same store and the same meter ([blob-shared-store](../blob-shared-store/SPEC.md)).
- Routes on one router: `_search` (through the coordinator when that feature is on), `_bulk`, `_doc`, `_create`, delete, and the point-in-time create, delete and list routes, the point-in-time service holding a 16 MiB buffer a request ([node-pit-route](../node-pit-route/SPEC.md)).
- The publish timer runs `WriterRole::publish_due` every `publish_every` under the lazy policy ([node-publish-timer](../node-publish-timer/SPEC.md)).
- The janitor timer runs `JanitorPass::run` every `janitor_every` over the role's shards ([node-janitor-pass](../node-janitor-pass/SPEC.md)); the sweep starts as a dry run, and `Config::sweep` is what each pass is told.
- Not here, listed: the transport handlers and links, the dead-node log collector, frozen search forwarding, the index admin routes, and the process entry point.

## Acceptance criteria

- AC-1: an assembled node answers the write routes (`PUT /{index}/_doc/{id}`, `_bulk`), the search route and the point-in-time routes on one listener: a write is 201, and a search and a point-in-time create of an index the node has no descriptor for are 404 `index_not_found`, so each route family is registered.
- AC-2: the writer role the assembly returns is the one the routes write through: a shard opened on it takes the writes, and the timer publishes it without a caller running a pass.
- AC-3: the point-in-time service's buffer is `oss_format::MAX_POINT_IN_TIME_BYTES` by default and is charged to the request-body pool: a pool smaller than that refuses a point-in-time create with 429 before anything is written.
- AC-4: a budget with no `open_indexes` bytes is refused at assembly, as `start_reader` refuses it.
- AC-5: the janitor timer sweeps the held shards: a blob of an older term nothing names is deleted without a caller running a pass, when the sweep is live.
- AC-6: a node starts with its janitor as a dry run: the same blob is kept through passes that would have deleted it.

## Test plan

`crates/node/tests/server/assemble.rs` over the emulator and a real server socket.

## S3 request budget

None at assembly beyond the stores' construction; every route's budget is its own spec's.

## Risks

- The assembly is the one place the roles meet, so a role added without its routes registered here answers 404; AC-1 lists the route families.
