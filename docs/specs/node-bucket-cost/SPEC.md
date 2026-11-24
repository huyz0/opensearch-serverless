# Requests of small indices in buckets

Status: measured on the in-process emulator with latency; provisional.

## Intent

The bucket switch stays off unless it costs no more than the default (the build rule for switches). This pins what a node asks of the store for 24 one-shard indices with the switch off and on, so the decision rests on counts.

## Acceptance criteria

- AC-1: with the switch on, a node started from configuration creates a one-shard index, writes a document, refreshes and searches it; the index's descriptor names a bucket whose manifest exists, and the shard has no register of its own. With the switch off the same requests leave a shard register and a descriptor with no bucket.
- AC-2: a search of a bucketed index makes the same number of requests as a search of an unbucketed one.
- AC-3: writing a document to each of 24 indices (refresh on) and 8 s of idleness each make within a tenth of the requests they make with the switch off.
- AC-4: creating a one-shard index in a bucket costs more than the descriptor alone but at most three times it.

## Measured (24 one-shard indices, one node, emulator with a latency profile; provisional)

| Phase | Off | On |
|---|---|---|
| create | 24 PUT | 48 GET, 48 PUT |
| write, refresh on | 255 GET, 24 GET-range, 139 PUT, 21 DELETE | 294 GET, 130 PUT, 21 DELETE |
| search | 38 GET, 24 GET-range, 1 PUT | 38 GET, 24 GET-range, 1 PUT |
| idle 8 s | 25 GET, 16 PUT | 25 GET, 16 PUT |

A create in a bucket adds two GETs and one PUT (the open bucket register, the manifest, one manifest swap). A write is about level (439 requests off, 445 on). Search and idle are equal.

## What this does not measure

Takeover of many small indices on one node's death, where one conditional write moves a whole bucket instead of one head per shard. That is where buckets are meant to win, and it is not measured here. Write p99 is not measured either.

## Decision

The create-time cost is an extra request per index with nothing measured on the other side, so the switch stays off by default.

## S3 request budget

Test only.
