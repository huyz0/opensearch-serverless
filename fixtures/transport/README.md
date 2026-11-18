# Transport recordings

What OpenSearch's own transport classes put on the wire, at the Java shell's version id (3.10.0, `3100099`): `StreamOutput`'s primitives, `DiscoveryNode`, and whole frames from `NativeOutboundMessage` (both handshakes, generic requests and responses, each uncompressed and compressed). `WireRecordings.java` is compiled against the OpenSearch 3.3.0 jars `fixtures/cluster-config/README.md` lists, in package `org.opensearch.transport` so it reaches the package-private messages. `crates/transport/tests/wire/` checks every row.

```bash
CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -d out fixtures/transport/WireRecordings.java
java -cp "$CP:out" org.opensearch.transport.WireRecordings fixtures/transport
```

| File | What |
|---|---|
| `stream.txt` | kind, value, bytes: 15 ints as `int` and `vint`, 9 `vlong`s, 12 `zlong`s and `long`s, booleans, 4 versions (wire id), 309 strings (UTF-8 hex: NUL, two- and three-byte chars, surrogate pairs, 300 characters, seeded random), optional strings, string arrays and a string map (comma-separated, UTF-8 hex), two task ids |
| `nodes.txt` | `with_attributes` or `plain`, then a node (name, id and ephemeral id as UTF-8 hex, host name, host address, IP, port, stream address IP or `-`, attributes `k=v;...`, roles `name:abbreviation:data,...`, wire version id), then its bytes |
| `exceptions.txt` | kind, description, bytes. `bare`: a failure a node answers (`security:message:status`, `rejected:message`, `not_found:action`, `status:message:status`), wrapped as `RemoteTransportException` from `w1` at 10.0.0.7:9300 for `internal:serverless/document/write`, without stack traces. `read`: chains as a Java node writes them, with stack traces, metadata and causes (`status`, `jdk:<key>`, `wrapped:<name>` for `NotSerializableExceptionWrapper`, `unknown:<id>`, `none`). `frame`: an error response frame for request id 42 |
| `frames.txt` | kind, request id, compressed, headers (`q:` request, `r:` response, `;`-separated, response values `|`-separated), features, action, message description (the node for a transport handshake response; `parent=` and `string=` for a generic message), then the frame |

The recordings come from 3.3.0's classes, not a build of the shell's 3.10 fork: every encoding here is the same across 3.x once the version gates are passed (a node's stream address from 3.2.0), and a version id is only a number on the wire. 
## Forwarded writes and the request MAC

`ForwardingRecordings.java` records what the shell's own classes (huyz0/OpenSearch `serverless/shell` at `429b4cbc`) write: `forwarding.txt` holds forwarded index and bulk requests and responses, each round-tripped through the class's `StreamInput` constructor and `writeTo`; `macs.txt` holds headers `TransportAuthenticator` signed (mac, key, action, recipient, other headers as `k=v` hex, the request's bytes, the header), each checked by the Java's own `verify`. Strings are UTF-8 hex, `~` is null and `-` empty. It compiles against the shell's sources beside the jars above: copy `forwarding/Forwarded{Index,Bulk}{Request,Response}.java`, `write/{BulkOperation,BulkOutcome,WriteHold,WriteOutcome}.java`, `nodelog/WalRecord.java`, `transport/TransportAuthenticator.java` and `metadata/TransportSecrets.java` into `src/` under their packages, with a stub `routing/ShardRouter.java` holding only `TOKEN_HEADER`.

```bash
javac -proc:none -nowarn -cp "$CP" -sourcepath src -d out fixtures/transport/ForwardingRecordings.java
java -cp "$CP:out" org.opensearch.serverless.transport.ForwardingRecordings fixtures/transport
```

`crates/transport/tests/wire/{forwarding,macs,mac_rotation}.rs` check every row.

`PushRecordings.java` records the reader-push messages the same way: `push.txt` holds commit push requests (with and without blocks, a writer and a parent task) and answers, newest-commit requests, and newest-commit answers (a commit, or none), each round-tripped through the shell's class. Hex is as above, a commit is its register value in hex, `~` is none. It compiles against the shell's sources and the 3.10 snapshot's jars (`build/distribution/local/.../lib/*.jar`) with the shell's source root as `-sourcepath`, in package `org.opensearch.serverless.transport`:

```bash
javac -proc:none -nowarn -cp "$CP" -sourcepath serverless/shell/src/main/java -d out fixtures/transport/PushRecordings.java
java -cp "$CP:out" org.opensearch.serverless.transport.PushRecordings fixtures/transport
```

`crates/transport/tests/wire/push.rs` checks every row.

## Live interop

`TransportInterop.java` runs a real `Netty4Transport` and `TransportService` at the shell's version id against `crates/transport/examples/interop.rs`, both ways: echo plain and compressed, `ActionNotFound` and status failures read as each side's own types, and a node under another ephemeral id refused. Beside the jars above it needs `transport-netty4-client`, Netty 4.1.125, the Lucene 10.3.1 modules, `opensearch-{cli,geo,secure-sm,telemetry,task-commons}`, joda-time, t-digest, HdrHistogram, spatial4j, jts-core and log4j-core, from Maven Central. Leave out `opensearch-compress`: its zstd provider needs a native library.

```bash
CP="$(ls jars/*.jar | grep -v opensearch-compress | tr '\n' ':')"
javac -proc:none -cp "$CP" -d out fixtures/transport/TransportInterop.java
cargo build -p oss-transport --example interop
coproc RS { target/debug/examples/interop serve; }; read -r PORT EPH <&"${RS[0]}"
java -cp "$CP:out" org.opensearch.transport.TransportInterop call "$PORT" "$EPH"; kill "$RS_PID"
coproc JV { java -cp "$CP:out" org.opensearch.transport.TransportInterop serve; }; read -r PORT EPH <&"${JV[0]}"
target/debug/examples/interop call "$PORT" "$EPH"; kill "$JV_PID"
```

Every line prints `ok`; a mismatch fails the run.
