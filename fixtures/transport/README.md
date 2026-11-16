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
| `frames.txt` | kind, request id, compressed, headers (`q:` request, `r:` response, `;`-separated, response values `|`-separated), features, action, message description (the node for a transport handshake response; `parent=` and `string=` for a generic message), then the frame |

The recordings come from 3.3.0's classes, not a build of the shell's 3.10 fork: every encoding here is the same across 3.x once the version gates are passed (a node's stream address from 3.2.0), and a version id is only a number on the wire. A running-node interop test comes with the client and server.
