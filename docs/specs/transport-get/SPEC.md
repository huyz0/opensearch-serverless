# The forwarded get message

## Serves

R7 cluster-wide reads: a `GET /{index}/_doc/{id}` that names a shard another node owns must reach that owner, because only the owner holds writes that are acknowledged and not yet published ([transport](../transport/SPEC.md), [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md)). This is the wire half; the owner's handler and the sender follow. A remote `_refresh` needs no message of its own: it is the empty bulk with the refresh flag the write path already forwards.

## Change

`oss-transport` gains `actions::read`: `GET_ACTION` (`internal:serverless/document/get`), `GetRequest` (parent task, index, optional index uuid, shard, id) and `GetResponse` (owner node id, id, found, optional source, sequence number, primary term, version, optional routing), laid out as the Java shell's `ForwardedGetRequest` and `ForwardedGetResponse` write them. The response carries the sequence identity a conditional write built on a forwarded get needs (the Java fixed exactly that bug).

## Acceptance criteria

- AC-1: a `GetRequest` writes the empty-or-named parent task, the index, an optional uuid, the shard as a vint and the id, and reads back whole; every shorter cut of it is an error.
- AC-2: a `GetResponse` writes the owner id, the id, found, an optional source, the sequence number as a zigzag long, the primary term as a vlong, the version as a zigzag long and an optional routing, and reads back whole.
- AC-3: a missing document's answer carries the unassigned identity (`-2`, `0`, `-1`) and round trips.
- AC-4: the action is named as the Java names it.

## Test plan

`crates/transport/tests/wire/read.rs`: byte layouts written out from the Java source (the empty task id is the empty string; the optional string is a presence byte), and round trips with every shorter cut refused. Not recorded from a build of the Java classes (no jars here), so the layout is provisional until `ForwardingRecordings.java` records it.

## S3 request budget

None: a codec.

## Risks

- The layout is read from the Java source, not recorded. A difference would show as a failed forwarded get between a Rust and a Java node.
