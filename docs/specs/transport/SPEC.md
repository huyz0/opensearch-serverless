# Transport: Rust and Java nodes speak one wire

## Serves

Roadmap R4.4 ([plan](../../roadmap.md)): node-to-node requests between Rust and Java nodes over OpenSearch's native transport, as the owner chose (2026-11-16). [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md). Built in parts, one PR each; this spec grows with them.

## Change

Part 1, the wire encodings (new crate `oss-transport`, layer 1):

- `stream`: `StreamOutput`'s and `StreamInput`'s encodings: byte, boolean, int, long, vint, vlong, zlong, strings (UTF-16 char count, then each char in one to three bytes), optional strings, string arrays, string maps, versions, `TaskId`, byte references; a writer bounded by its limit and a reader that refuses truncated, overlong and malformed values.
- `version`: the shell's version (3.10.0, wire id `3100099 ^ 0x08000000`), its minimum compatible version (2.19.0), and the rule that a peer is accepted only at a version the fixtures cover.
- `frame`: the 23-byte fixed header (`ES`, length, request id, status, version, variable-header length), the variable header (request headers, response headers, and for a request the features and action), keep-alive pings, and raw-deflate compression behind `DFL\0`, inflating to a bound.
- `handshake`: the TCP handshake (`internal:tcp/handshake`) and the transport handshake (`internal:transport/handshake`) request and response messages; `node`: `DiscoveryNode` and `TransportAddress` written with or without attributes, roles with their abbreviations and data flag.
- Not yet: sockets, the client, the server, exceptions, forwarded actions and the MAC (parts 2 and 3).

## Acceptance criteria

- AC-1: every recorded primitive (15 ints as int and vint, 9 vlongs, 12 zlongs and longs, booleans, 4 versions, 309 strings with NUL, two- and three-byte chars and surrogate pairs, optional strings, string arrays, a string map, two task ids) encodes to the Java's bytes and decodes back to its value; a peer version other than 3.10.0 is refused.
- AC-2: the decoder refuses a truncated value at every cut of every recorded encoding, a vint longer than five bytes, a negative length, a string whose bytes are not a valid char sequence, and a value past the writer's limit.
- AC-3: each of 5 recorded `DiscoveryNode`s (IPv4, IPv6, a stream address, every built-in role and a dynamic one, non-ASCII ids, the minimum compatible version) encodes with and without attributes to the Java's bytes and decodes to its fields.
- AC-4: each uncompressed recorded frame (both handshakes each way, five transport handshake responses, requests with a header, features and a parent task, a response with a two-value response header) encodes to the Java's bytes, and every recorded frame, compressed or not, decodes to the same fields and message; a frame Rust compresses decodes to the same message.
- AC-5: a frame header with a bad marker, a length below the fixed header or past the bound, a variable header past 64 KiB or longer than its frame, a frame not the length it announces, or a compressed message whose output passes the bound is refused; a ping (`ES` and length -1) reads as a ping.

## Test plan

- AC-1 to AC-5: `crates/transport/tests/` over `fixtures/transport/`, recorded by `WireRecordings.java` with OpenSearch 3.3.0's own classes at the shell's version id. Fails if any byte differs.
- Mutants over the diff; planted defect: a vint's continuation bit tested on the wrong byte.

## S3 request budget

| Operation | Requests |
|---|---|
| encode or decode anything here | 0 |

## Switch

None: a codec with nothing to compare against, making no requests and serving nothing yet.

## Risks

- The recordings come from OpenSearch 3.3.0's jars at 3.10.0's version id, not from a build of the shell's 3.10 fork. The encodings recorded are stable across 3.x where the version gates (`V_3_2_0` for a node's stream address) are passed; part 2's interop test against a running shell node closes the gap.
