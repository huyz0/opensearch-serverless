# Mixed fleet: Java opens and takes back a shard a Rust writer took

`store/` is the object store after a Rust writer (node `node-2`, term 3) took the shard `fixture` from the Java shell's recorded store (`../shell-blobs`: a commit at term 2, with a segment in a node pack and one in a segment blob, and a head naming the dead Java node), wrote one document and published. `crates/node/tests/mixed_takeover.rs` makes it: run with `OSS_MIXED_OUT=<dir>` and the second test copies the store there as the filesystem store lays it out (registers are an 8-byte generation, then the value). The segments carry random ids, so a new run gives new bytes.

`JavaTakesBack.java` then plays the Java node over it, with the shell's own classes from huyz0/OpenSearch `feature/serverlessshell` at `334bd9bc`, unchanged:

- `CommitManifest.fromStream` reads the Rust commit register, `FileLocation.parse` each file's place (a node pack of the Java node for three files, the segment blob of term 2 and the Rust blob of term 3 for the rest), and a Lucene `DirectoryReader` through `SoftDeletesDirectoryReaderWrapper` opens them: 6 live documents, the Java's five and the Rust's one.
- `ShardHeadStore.read` reads the head the Rust writer left (term 3, owner `node-2`, the Java node in `writers`), and, with the clock past its lease and an oracle that finds no node live, `acquire` takes it at term 4 and names `node-2` among the writers.

`java.txt` is what it printed. `crates/node/tests/mixed_java_reads.rs` opens the same store with the Rust reader and checks the same numbers and the head.

`stub/` holds three classes reduced to what these calls need (the server's mappers and blob store are not compiled): `PruningDigest` (the commit has no digest), `BlobContainer` and `BlobStore`. The rest of the server's blob store classes are used as they are, with one line changed in `BlobRegister`: `Randomness.createSecure()` is `new SecureRandom()`. `JavaTakesBack` ends with a small directory-backed `ConditionalBlobContainer` that keeps registers as the filesystem store does.

```bash
O=<OpenSearch checkout at 334bd9bc>
S=out-src/org/opensearch/common/blobstore; mkdir -p $S out-src/org/opensearch/cluster/routing
for f in BlobPath BlobMetadata DeleteResult BlobRegister BlobRegisterCasResult ConditionalBlobContainer; do cp $O/server/src/main/java/org/opensearch/common/blobstore/$f.java $S/; done
sed -i 's/org.opensearch.common.Randomness.createSecure()/new java.security.SecureRandom()/' $S/BlobRegister.java
cp $O/server/src/main/java/org/opensearch/cluster/routing/Murmur3HashFunction.java out-src/org/opensearch/cluster/routing/
cp -r fixtures/mixed-takeover/stub/* out-src/
CP="$(ls jars/*.jar | tr '\n' ':')"   # the jars fixtures/node-log/README.md lists
javac -proc:none -nowarn -cp "$CP" \
  -sourcepath "out-src:$O/serverless/shell/src/main/java:$O/libs/core/src/main/java:$O/libs/common/src/main/java:$O/libs/x-content/src/main/java" \
  -d out fixtures/mixed-takeover/JavaTakesBack.java $O/libs/x-content/src/main/java/org/opensearch/common/xcontent/spi/XContentProvider.java
mkdir -p out/META-INF/services
cp $O/libs/x-content/src/main/resources/META-INF/services/org.opensearch.core.xcontent.spi.MediaTypeProvider out/META-INF/services/
cp -r fixtures/mixed-takeover/store /tmp/run   # the program writes the head it takes
java -cp "$CP:out" org.opensearch.serverless.store.JavaTakesBack /tmp/run
```
