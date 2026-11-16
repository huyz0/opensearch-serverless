# Node-lease recordings

Node leases, lease digests and members indexes written by the shell's own classes: `NodeLease`, `LeaseDigest` and `NodeLogEpoch` from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/{membership,store}/` at `3920dc36`, compiled unchanged against the OpenSearch 3.3.0 jars that `fixtures/cluster-config/README.md` lists. The members index is `BlobLeaseMembership.encodeIds`, copied into `NodeLeases.java`, because that class needs the fork's register container to compile. `crates/format/tests/node_leases.rs` reads the same inputs in Rust and checks it gets the same leases and bytes.

```bash
mkdir -p src/org/opensearch/serverless/membership src/org/opensearch/serverless/store
for f in membership/NodeLease membership/LeaseDigest store/NodeLogEpoch; do
  git -C <OpenSearch checkout> show 3920dc36:serverless/shell/src/main/java/org/opensearch/serverless/$f.java \
    > src/org/opensearch/serverless/$f.java
done
CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -d out src/org/opensearch/serverless/*/*.java fixtures/node-leases/NodeLeases.java
java -cp "$CP:out" org.opensearch.serverless.membership.NodeLeases fixtures/node-leases
```

| File | What |
|---|---|
| `cases.txt` | leases to read, one a line, written by hand: every field, unknown fields, blank names and zones, unsorted and repeated roles, node-log epochs, escapes, and leases the Java refuses |
| `parsed.txt` | for each case, what `toBytes` writes for the lease read, checked to read back to the same bytes, or `refused` |
| `strict.txt` | leases the Java reads by coercing a JSON type it never writes there (a number as text, text as a number or a flag, a scalar where an array goes, an element that is not an epoch, `null`), or refuses (a repeated field, an `int` past its range), and a lease followed by other bytes; the Rust refuses each |
| `strict-parsed.txt` | what the Java makes of each |
| `digests.txt` | for each of four fleets (none; case 1; cases 1 to 3; cases 2, 8, 12, 14 and 24), `LeaseDigest.encode` of the leases as read, written at `1700000000000` plus the fleet's number, in hex |
| `indexes.txt` | for the same fleets, the members index of their ids, in hex |
