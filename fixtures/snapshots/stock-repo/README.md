# A whole stock repository directory

`GenRepo.java` writes, with the server's own formats (`SNAPSHOT_FORMAT`, `INDEX_METADATA_FORMAT`, `INDEX_SHARD_SNAPSHOT_FORMAT`), the directory `repo/`: `index.latest`, `index-1`, one snapshot `snap-1` of one index `logs` of one shard, whose two files (`_0.cfs`, 3,000 bytes stored in two 2,048-byte parts, and `segments_1`, 100 bytes whole) end in a real Lucene footer. The files are not a Lucene index; they let the restore's copy and its length and CRC checks be tested byte for byte. The recorded 3.10 snapshot that answers a search is `../stock/README.md`.

```
CP=<the libs/*/build/distributions jars, server/build/distributions/opensearch-3.10.0-SNAPSHOT.jar and the gradle cache jars, lucene-core included>
javac -proc:none -cp "$CP" -d /tmp/genrepo GenRepo.java && java -cp "$CP:/tmp/genrepo" GenRepo fixtures/snapshots/stock-repo/repo
```

Test: `crates/node/tests/server/stock_restore.rs`.
