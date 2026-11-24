# Concurrent write requests

## Serves

The write path of [node-bulk-route](../node-bulk-route/SPEC.md) and [node-doc-write-route](../node-doc-write-route/SPEC.md), and the group commit of [node-log-committer](../node-log-committer/SPEC.md): requests to different shards share a commit window instead of waiting for one each.

## Finding

Found while measuring `OSS_STORE_USAGE` (crates/node/tests/store_usage_measure.rs): with the default 250 ms window, one client's bulk takes 254 ms, but 2, 4 and 8 clients take 510, 1,016 and 2,035 ms, and a node completes about 4 bulks a second however many clients send. The cause is that `_bulk` and the single-document routes hold the node's one `IdGenerator` mutex for the whole request, including the wait for the log commit, so every write request on a node runs alone and gets a window to itself.

A second ceiling showed once the first was gone: with a bulk of its own for each of 16 and 64 clients, bulks took 1,016 and 4,079 ms (4 and 16 windows), because the node's bulk budget allowed 4 slices at once (`bulk_holders`, sized as "no more than cores finish sooner") and a slice holds its grant across the commit wait, so a node took 4 bulks a window.

## Change

- `oss_node::id_lanes::IdLanes` hands each write request its own `IdGenerator`, so no lock is held while a request runs. A request's generator carries the node's address with its last two bytes replaced by the request's lane (a counter that wraps at 65,536) and a sequence seeded from the same counter, so two requests less than 65,536 requests apart never give the same id, and the ids of one request stay time-ordered as before.
- `BulkService` and `DocService` take their generator from `IdLanes` and hold no lock across the request.

- The node's default `bulk_holders` is 64 (was 4): the bytes of the budget still bound the heap; holders only bounded applies in progress, and a slice now waits a window while holding its grant.

## Acceptance criteria

- AC-1: generators from one `IdLanes` give no id twice, across 64 generators drawing 4,096 ids each at one clock value and across a wrap of the lane counter.
- AC-2: a generator's ids have the layout of before (20 characters, base64url) and are in time order within one request.
- AC-3: 16 clients each sending a bulk to an index of their own at the same time finish within 2.5 seconds with the default 250 ms window (serialised, they need 4 seconds); and the same for 16 single-document writes.
- AC-4: auto ids of two concurrent bulks to one index are all distinct (the documents counted equal the documents sent).
- AC-5: 32 clients each sending a bulk to an index of their own at the same time finish within 1.2 seconds (four holders at a time need 2 seconds).

## Test plan

`crates/node/tests/id_lanes.rs` (AC-1, AC-2); `crates/node/tests/server/boot_write_concurrency.rs` (AC-3, AC-4, AC-5): a booted node over a file store. AC-3 was seen failing before the change.

## S3 request budget

Unchanged in kind; more writes share one log blob, so a node under concurrent writes makes fewer PUTs for the same writes.

## Switch

None: no downside. Throughput and latency under concurrent writes improve; one client is unchanged.
