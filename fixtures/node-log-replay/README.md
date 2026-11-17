# Node-log replay recording

A node's write log as the shell writes it on a filesystem store, and what the shell's own successor reads from it. `crates/writer/tests/node_log_cross_replay.rs` fences and replays a copy of `log/` with the Rust takeover on `FsStore` and checks it gets the same.

`NodeLogReplay.java` runs the shell's `NodeLogCommitter`, `NodeLogStore` and `NodeLogTakeover` from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/nodelog/` at `429b4cbc`, unchanged, as node `node-1` from epoch 7 with no window:

- epoch 7: `a0` (shard `logs`/0), `b0` (`logs`/1), then one blob `logs`/0, `metrics`/0 and `logs`/1 share (`a1`; `c0` and its deletion; `b1`), then a fence by `someone` at term 1 closes the epoch and the next write is refused;
- epoch 8: `a2` (characters past U+FFFF and escapes), `b2` (a 5,000-byte source), `c2` (term 2);
- `logs`/0 published through the shared blob, `logs`/1 through `b0`; the lease is what `positions()` then reports.

`node-2` at term 2 then fences every epoch and reads each shard, and a shard the log never named.

| File | What |
|---|---|
| `log/` | every blob the committer wrote, at its key |
| `lease.txt` | the epochs the takeover is given: epoch, trim, landed |
| `replay.txt` | `ends()`, then for each shard `maxSeqNoOf`, `holds` and each record's `toBytes`, in the order `forEachRecord` gives them |
| `fences.txt` | each blob the takeover wrote, at its key, in hex |

`stub/` holds the three server interfaces and the meter the node log calls, reduced to those calls: `BlobContainer`, `BlobPath` and `BlobStore` (the store is a directory per path component and a file per blob, as the server's `FsBlobContainer` lays it out, taken slots refused by `CREATE_NEW`), and `StoreUsage`, which charges no one. `SequenceNumbers` and `Versions` come from `../node-log/stub/`. It needs the jars `../node-log/README.md` lists.

```bash
O=<OpenSearch checkout at 429b4cbc>
mkdir -p src/org/opensearch/serverless/nodelog
for f in $(git -C $O ls-tree --name-only 429b4cbc serverless/shell/src/main/java/org/opensearch/serverless/nodelog/ \
    | grep -v -e NodeLogSettings -e package-info); do
  git -C $O show 429b4cbc:$f > src/org/opensearch/serverless/nodelog/$(basename $f)
done

CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" \
  -sourcepath "fixtures/node-log/stub:fixtures/node-log-replay/stub:$O/libs/core/src/main/java:$O/libs/common/src/main/java:$O/libs/x-content/src/main/java" \
  -d out src/org/opensearch/serverless/nodelog/*.java fixtures/node-log-replay/NodeLogReplay.java \
  $O/libs/x-content/src/main/java/org/opensearch/common/xcontent/spi/XContentProvider.java
mkdir -p out/META-INF/services
cp $O/libs/x-content/src/main/resources/META-INF/services/org.opensearch.core.xcontent.spi.MediaTypeProvider out/META-INF/services/
rm -rf fixtures/node-log-replay/log
java -cp "$CP:out" org.opensearch.serverless.nodelog.NodeLogReplay fixtures/node-log-replay
```

Two runs give the same files: the committer runs with no window and one writer, so every blob's slot and content follow from the calls.
