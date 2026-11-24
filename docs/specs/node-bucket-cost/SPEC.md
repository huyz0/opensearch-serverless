# Requests of small indices in buckets

Status: measured on the in-process emulator with latency; provisional.

## Intent

The bucket switch stays off unless it costs no more than the default (the build rule for switches). This pins what a node asks of the store for 24 one-shard indices with the switch off and on, so the decision rests on counts.

## Acceptance criteria

- AC-1: with the switch on, a node started from configuration creates a one-shard index, writes a document, refreshes and searches it; the index's descriptor names a bucket whose manifest exists, and the shard has no register of its own. With the switch off the same requests leave a shard register and a descriptor with no bucket.
- AC-2: a search of a bucketed index makes the same number of requests as a search of an unbucketed one.
- AC-3: writing a document to each of 24 indices (refresh on) and 8 s of idleness each make within a tenth of the requests they make with the switch off.
- AC-5: a second node taking over 24 one-shard indices after the first is killed makes fewer requests, and fewer PUTs, with the switch on.
- AC-4: creating a one-shard index in a bucket costs more than the descriptor alone but at most three times it.

## Measured (24 one-shard indices, one node, emulator with a latency profile; provisional)

| Phase | Off | On |
|---|---|---|
| create | 24 PUT | 48 GET, 48 PUT |
| write, refresh on | 255 GET, 24 GET-range, 139 PUT, 21 DELETE | 294 GET, 130 PUT, 21 DELETE |
| search | 38 GET, 24 GET-range, 1 PUT | 38 GET, 24 GET-range, 1 PUT |
| idle 8 s | 25 GET, 16 PUT | 25 GET, 16 PUT |

A create in a bucket adds two GETs and one PUT (the open bucket register, the manifest, one manifest swap). A write is about level (439 requests off, 445 on). Search and idle are equal.

### Takeover (`bucket_takeover_cost.rs`)

24 one-shard indices written on n1, n2 started, n1 killed, then a document written to every index through n2 until each accepts it. The grouped runs name a group at create with `index.serverless.bucket_group` (the shell's rule: the setting, else the index name), here four tenants of six indices:

| | Off | On, four tenants | On, each its own group |
|---|---|---|---|
| requests | 341 GET, 26 GET-range, 4 HEAD, 138 PUT, 7 DELETE (516) | 351 GET, 3 GET-range, 4 HEAD, 70 PUT, 7 DELETE (435) | 371 GET, 3 GET-range, 4 HEAD, 81 PUT, 7 DELETE (466) |
| time | 9.3 s | 9.2 s | 9.3 s |

Grouped by tenant, a takeover makes 16% fewer requests and 49% fewer PUTs in the same time; with every index its own group it makes 10% fewer requests and 41% fewer PUTs.

## What this does not measure

Write p99, and a cluster-level `serverless.bucket` policy at create (a create reads the index setting and the node switch only).

## Decision

Per index, a bucket costs three more requests at create and, grouped by tenant, saves about three at one takeover: break even over a life with one takeover, a gain with more, and a loss for an index never taken over. That is not "no extra S3 requests", so the switch stays off by default. A create now takes the shell's `index.serverless.bucket_group` and `index.serverless.bucket` (auto or off), so a deployment that wants buckets can turn the node switch on and group by tenant.
