# Stock repository metadata envelopes

`GenEnvelope.java` writes the envelope a stock OpenSearch repository puts round each metadata blob (`ChecksumBlobStoreFormat`): Lucene's `CodecUtil` header (codec name, version 1), a Jackson SMILE body, optionally in the `DFL\0` deflate framing of `DeflateCompressor`, and `CodecUtil`'s footer. `snapshot.plain.dat` and `meta.plain.dat` carry the codecs `snapshot` and `metadata`; `snapshot.deflate.dat` is the compressed form; `body.smile` is the bare body.

```
CP=lucene-core-10.5.1.jar:jackson-core-3.2.3.jar:jackson-dataformat-smile-3.2.3.jar
javac -cp $CP GenEnvelope.java && java -cp $CP:. GenEnvelope fixtures/snapshots/stock-envelope
```

They are not blobs a stock node wrote; a recorded stock snapshot joins them (`fixtures/snapshots/stock/README.md`). Test: `crates/format/tests/stock_envelope.rs`.
