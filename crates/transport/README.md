# oss-transport

OpenSearch's native transport, the wire Java shell nodes forward writes, searches, pushes and cache warm-ups over, ported so Rust and Java nodes talk to each other directly ([ADR 0016](../../docs/adr/0016-native-transport-and-early-revoke.md)).

## Owns
The stream encodings of OpenSearch's `StreamInput` and `StreamOutput` (vint, vlong, zlong, strings, string arrays and maps, versions, task ids), the shell's wire version and its compatibility rule, the transport frame (fixed header, variable header with thread-context headers, features and action, keep-alive pings, deflate compression), the TCP and transport handshake messages, and `DiscoveryNode` as the transport handshake writes it. Next: the client, the server, exceptions, the forwarded actions and the Java's request MAC.

## Depends on
`miniz_oxide` (raw deflate, pure Rust). No workspace crate yet; `oss-mem` comes with the server's frame reservations.

## Entry points
`src/lib.rs`: `stream::{Output, Input}`, `version::WireVersion`, `frame::{FrameHeader, encode_request, encode_response, decode}`, `handshake`, `node::DiscoveryNode`. Tested against Java-recorded bytes (`tests/`, `fixtures/transport/`). Spec: [docs/specs/transport](../../docs/specs/transport/SPEC.md).
