# A snapshot recorded from a stock OpenSearch 3.10 node

`repo/` is the fs repository a stock OpenSearch `3.10.0-SNAPSHOT` node (the `opensearch-min` tarball built from the shell's OpenSearch checkout, run as a single node) wrote: one index `logs` (one shard, no replicas, a `title` text, a `level` keyword and an `n` integer; the twenty documents of `bulk.ndjson`, force-merged to one segment), one snapshot `snap-1` with `include_global_state` false. Its blobs are the node's own, byte for byte; nothing was edited.

Recorded with:

```
PUT /logs   {"settings":{"number_of_shards":1,"number_of_replicas":0},"mappings":{"properties":{"title":{"type":"text"},"level":{"type":"keyword"},"n":{"type":"integer"}}}}
POST /logs/_bulk?refresh=true      (bulk.ndjson)
POST /logs/_forcemerge?max_num_segments=1&flush=true
PUT /_snapshot/stock               {"type":"fs","settings":{"location":"<dir>"}}
PUT /_snapshot/stock/snap-1?wait_for_completion=true   {"indices":"logs","include_global_state":false}
```

The stock node's own answers, the ones the restore test holds the restored index to:

| Search | Answer |
|---|---|
| `{"size":0}` | 20 hits |
| `match` `title: gamma`, sorted by `n` | 4 hits, ids `2`, `7`, `12`, `17` |
| `term` `level: warn`, `size` 0 | 10 hits |

The recording is the reason a restore drops `index.replication.type: DOCUMENT`: a 3.10 node writes it on every index.

Test: `crates/node/tests/server/stock_restore_search.rs`; its request counts are `snapshot.stock_restore.requests` and `snapshot.stock_restore_cleanup.requests` in [docs/request-budgets.md](../../../docs/request-budgets.md).
