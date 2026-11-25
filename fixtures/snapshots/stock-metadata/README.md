# Stock index metadata and shard snapshot blobs

`GenMetadata.java` writes, with the server's own `BlobStoreRepository.INDEX_METADATA_FORMAT` and `INDEX_SHARD_SNAPSHOT_FORMAT` (`ChecksumBlobStoreFormat.serialize`), an index's metadata blob (codec `index-metadata`) and a shard's snapshot blob (codec `snapshot`), each plain and deflate-compressed (`DeflateCompressor`): `index-metadata.plain.dat`, `index-metadata.deflate.dat`, `snapshot.plain.dat`, `snapshot.deflate.dat`. The index has two shards, a mapping with a text, a keyword and a long field, and an alias; the shard snapshot lists a 12 MB file stored in 5 MB parts with an inline copy, and a 300-byte file.

```
CP=<the libs/*/build/distributions jars, server/build/distributions/opensearch-3.10.0-SNAPSHOT.jar and the gradle cache jars>
javac -proc:none -cp "$CP" GenMetadata.java && java -cp "$CP:." GenMetadata fixtures/snapshots/stock-metadata
```

Test: `crates/format/tests/stock_shard_snapshot.rs`. They are blobs the server's formats wrote, not blobs a running node put in a repository; a recorded stock snapshot joins them (`fixtures/snapshots/stock/README.md`).
