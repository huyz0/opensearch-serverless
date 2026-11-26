# Stock repository metadata envelopes

`GenEnvelope.java` writes the envelope a stock OpenSearch repository puts round each metadata blob (`ChecksumBlobStoreFormat`): Lucene's `CodecUtil` header (codec name, version 1), a Jackson SMILE body, optionally in the `DFL\0` deflate framing of `DeflateCompressor`, and `CodecUtil`'s footer. `snapshot.plain.dat` and `meta.plain.dat` carry the codecs `snapshot` and `metadata`; `snapshot.deflate.dat` is the compressed form; `body.smile` is the bare body.

```
CP=lucene-core-10.5.1.jar:jackson-core-3.2.3.jar:jackson-dataformat-smile-3.2.3.jar
javac -cp $CP GenEnvelope.java && java -cp $CP:. GenEnvelope fixtures/snapshots/stock-envelope
```

They are not blobs a stock node wrote; a recorded stock snapshot joins them (`fixtures/snapshots/stock/README.md`). Test: `crates/format/tests/stock_envelope.rs`.

## The writers' output, read by Lucene and the server

`CheckStockWrite.java` reads what the Rust writers left (`OSS_WRITE_STOCK=dir`) with Lucene's `CodecUtil` and the server's `RepositoryData.snapshotsFromXContent`, against the jars of a 3.10 node's `lib/`:

```
OSS_WRITE_STOCK=/tmp/stw cargo test -p oss-format --test stock_envelope_write --test stock_repository_data_write
javac -proc:none -cp "$L/*" -d /tmp/cstw CheckStockWrite.java
java -cp "$L/*:/tmp/cstw" CheckStockWrite /tmp/stw
```
