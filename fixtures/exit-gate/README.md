# R6 exit gate: Java and Rust open each other's commits

`rust-commit/` is a shard the Rust writer indexed (three documents, then one written again and one deleted) and published twice with `oss_writer::publish::ShardPublisher`: `store/` is the object store as the publisher left it (segment blobs, the commit register, the index descriptor), `manifest.bin` the last commit register's value. `crates/node/examples/write_commit.rs` writes it:

```bash
cargo run -p oss-node --example write_commit -- fixtures/exit-gate/rust-commit
```

`JavaOpensRust.java` opens it with the shell's own classes: `CommitManifest.fromStream` for the register's value, `FileLocation.parse` for each file's blob range, and a Lucene `DirectoryReader` through `SoftDeletesDirectoryReaderWrapper`. It compiles against the 3.10 snapshot's jars with the shell's source root as `-sourcepath`:

```bash
CP="$(ls build/distribution/local/opensearch-3.10.0-SNAPSHOT/lib/*.jar | tr '\n' ':')"
javac -proc:none -nowarn -cp "$CP" -sourcepath serverless/shell/src/main/java -d out fixtures/exit-gate/JavaOpensRust.java
java -cp "$CP:out" org.opensearch.serverless.store.JavaOpensRust fixtures/exit-gate/rust-commit
```

`rust-commit/java.txt` is what it printed: the term, writer and file count of the manifest, and the live (`num_docs`) and total (`max_doc`) documents. `crates/node/tests/exit_gate.rs` opens the same bytes with the Rust reader and asserts the same numbers. Re-run the example and the Java check together: the segments carry random ids, so the bytes differ run to run.

The other direction, the Rust reader opening what the Java shell wrote, is `fixtures/compat-blobs` and `fixtures/shell-blobs`, read by `the_reader_node_answers_every_search_fixture_as_opensearch_did` and the reader's open tests.
