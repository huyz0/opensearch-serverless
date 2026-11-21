# A forwarded get carries `realtime` to a peer that reads it

## Serves

A get with `realtime=false` of a shard another node owns was answered realtime, because the Java `ForwardedGetRequest` has no field for it. Builds on [node-read-forward](../node-read-forward/SPEC.md). Not here: `_routing` stored and returned by the engine (its own change).

## Change

- `GetRequest` gains `realtime` (default true). `write` is the Java layout whatever it holds; `write_extended` adds one trailing byte only when it is false. `read` takes the byte when there is one and reads realtime otherwise.
- A node publishes `oss-rust/<version>` as its lease's build version. `GetForwarder` writes the extended layout only to a peer whose lease says so; to any other peer the Java layout, so a Java owner never sees the byte.
- The owner handler reads with the request's `realtime`; `Routes::get_remote_as` carries it from `get_owned`.
- The limit is in [compatibility](../../compatibility.md).

## Acceptance criteria

- AC-1: a realtime request is the Java layout byte for byte in both writes, and a non-realtime request adds exactly one trailing zero byte in `write_extended` only; `read` of either gives the right flag.
- AC-2: forwarded to a peer of this project, a get that is not realtime does not see an unrefreshed write and a realtime one does.
- AC-3: forwarded to a peer whose lease does not say so (no version, or another), a get that is not realtime is answered, realtime.

## Test plan

`crates/transport/tests/wire/read.rs` and `crates/node/tests/read_forward.rs`, over a real socket.

## S3 request budget

None. LIST 0, HEAD 0, PUT 0.

## Risks

- A mixed Java and Rust fleet reads realtime for `realtime=false` of a remote shard; documented.
