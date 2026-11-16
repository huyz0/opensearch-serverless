# oss-transport

OpenSearch's native transport, the wire Java shell nodes forward writes, searches, pushes and cache warm-ups over, ported so Rust and Java nodes talk to each other directly ([ADR 0016](../../docs/adr/0016-native-transport-and-early-revoke.md)).

## Owns
The stream encodings of OpenSearch's `StreamInput` and `StreamOutput` (vint, vlong, zlong, strings, string arrays and maps, versions, task ids), the shell's wire version and its compatibility rule, the transport frame (fixed header, variable header with thread-context headers, features and action, keep-alive pings, deflate compression), the TCP and transport handshake messages, `DiscoveryNode` as the transport handshake writes it, exceptions as `writeException` writes them, and the blocking client and server (frames held in reserved memory, failures told apart). Next: the forwarded actions and the Java's request MAC.

## Depends on
`oss-mem` (frames are reserved from `Pool::Transport`), `miniz_oxide` (raw deflate, pure Rust).

## Entry points
`src/lib.rs`: `server::{Server, ServerConfig, Handler}`, `client::{Client, Call, CallError, PeerId}`, `exception::RemoteException`, `stream::{Output, Input}`, `version::WireVersion`, `frame::{encode, decode}`, `handshake`, `node::DiscoveryNode`. Tested against Java-recorded bytes (`tests/`, `fixtures/transport/`), over loopback, and against a live Java transport by hand (`examples/interop.rs`). Spec: [docs/specs/transport](../../docs/specs/transport/SPEC.md).
