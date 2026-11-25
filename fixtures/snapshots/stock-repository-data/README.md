# A stock repository's `index-N`

`GenRepositoryData.java` builds a `RepositoryData` with the server's own classes (two snapshots over two indices, with metadata generations and identifiers) and writes `index-7.json` with `RepositoryData.snapshotsToXContent`, the writer a stock node uses:

```
CP=<the libs/*/build/distributions jars, server/build/distributions/opensearch-3.10.0-SNAPSHOT.jar and the gradle cache jars>
javac -cp "$CP" GenRepositoryData.java && java -cp "$CP:." GenRepositoryData fixtures/snapshots/stock-repository-data
```

Test: `crates/format/tests/stock_repository_data.rs`. It is not an `index-N` a running node wrote after a snapshot request; a recorded stock snapshot joins it (`fixtures/snapshots/stock/README.md`).
