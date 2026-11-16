# Transport: Rust and Java nodes speak one wire

## Serves

Roadmap R4.4 ([plan](../../roadmap.md)): node-to-node requests between Rust and Java nodes over OpenSearch's native transport, as the owner chose (2026-11-16). [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md). Built in parts, one PR each; this spec grows with them.

## Change

Part 1, the wire encodings (new crate `oss-transport`, layer 1):

- `stream`: `StreamOutput`'s and `StreamInput`'s encodings: byte, boolean, int, long, vint, vlong, zlong, strings (UTF-16 char count, then each char in one to three bytes), optional strings, string arrays, string maps, versions, `TaskId`, byte references; a writer bounded by its limit and a reader that refuses truncated, overlong and malformed values.
- `version`: the shell's version (3.10.0, wire id `3100099 ^ 0x08000000`), its minimum compatible version (2.19.0), and the rule that a peer is accepted only at a version the fixtures cover.
- `frame`: the 23-byte fixed header (`ES`, length, request id, status, version, variable-header length), the variable header (request headers, response headers, and for a request the features and action), keep-alive pings, and raw-deflate compression behind `DFL\0`, inflating to a bound.
- `handshake`: the TCP handshake (`internal:tcp/handshake`) and the transport handshake (`internal:transport/handshake`) request and response messages; `node`: `DiscoveryNode` and `TransportAddress` written with or without attributes, roles with their abbreviations and data flag.

Part 2, the sockets (same crate, now depending on `oss-mem`):

- `exception`: `writeException` and `readException`: a node's failure wrapped as `RemoteTransportException` (`[name][ip:port][action]`, its address and action), written for `OpenSearchSecurityException`, `OpenSearchStatusException`, `ActionNotFoundTransportException` and `OpenSearchRejectedExecutionException`, without stack traces; any recorded chain read (OpenSearch kinds by registry id, the JDK and Lucene kinds 1 to 18), up to 16 deep, with stack frames, suppressed exceptions, headers and metadata skipped; a subclass whose own fields are unknown stops the read, marked incomplete. The HTTP status a failure maps to.
- `channel`: reads whole frames into memory reserved from `Pool::Transport`: the variable header, then a precheck of the headers, then the message; a compressed message inflates within what the pool can spare. A frame past the bound or more than the pool holds is read through a fixed buffer and dropped, answered 413 or 429, and the channel stays open.
- `server`: one accept thread, a reader thread a channel, a fixed pool of workers. Answers the TCP handshake (refusing a peer whose minimum version is not 2.19.0 or 3.10.0), the transport handshake with the node, pings with a ping, registered actions through their handlers, an unknown action `ActionNotFoundTransportException`, a full queue a rejection, a request at another version 400. Channels are capped; one that sends no whole frame within 10 s of connecting, or nothing for 5 minutes, is closed; stopping closes them all.
- `client`: up to 4 sockets a peer and a global socket cap, each multiplexing requests by id with one reader thread; both handshakes within the caller's deadline, refusing a node under another ephemeral id or version. Failures told apart: `Refused` (ECONNREFUSED only), `Timeout`, `Io`, `Busy`, `Remote`, `Unauthorized` (a security failure, 401 or 403), `TooLarge`, `WrongPeer`. A pooled socket to another process at the address is closed, never reused.
- Not yet: early revoke (part 4).

Part 3, the forwarded writes and their authentication:

- `actions::write` (`oss-transport`): `ForwardedIndexRequest` and `ForwardedBulkRequest` (`internal:serverless/document/write` and `/bulk`) and their responses, as the shell's `writeTo` and `StreamInput` constructors write and read them, byte for byte, because the MAC digests the receiver's re-serialization. A bulk's count is checked against the bytes left before anything is sized. The client writes a request whole, its parent task id first, as each action's `writeTo` does.
- `auth` (`oss-transport`): the shell's `TransportAuthenticator`. A request carries `x-serverless-transport-mac`, `generation:sender:timestamp:nonce:mac`, an HMAC-SHA256 keyed by the current secret over a tag, the action, sender, recipient (`node_id/ephemeral_id`), timestamp and nonce, then a SHA-256 of the request's bytes and one of its other headers in key order. The receiver accepts a timestamp within one lease TTL, each nonce once (65,536 remembered; a full cache refuses rather than evicts), and the previous key only for one TTL after a rotation to the next generation; a newer generation re-reads the secret at most once a second and answers 429 meanwhile; any other failure is a security refusal (403). `Guarded` wraps a handler with it. Unlike a Java node, a Rust node never accepts the legacy bare token.
- `transport_secret` (`oss-cluster`, which now depends on `oss-transport`) and `TransportSecret` (`oss-format`): the `cluster/config/transport-secret` register the shell reads, a JSON value or a bare secret, at the register's generation. A deployment's first node mints a 32-byte key with one conditional create, as a Java node does; a node that loses the create reads the winner's. Each node caches it and re-reads once a lease TTL, when its clock goes back, or when the authenticator meets an unseen generation; one caller reads while the others use the secrets held, and a failed read keeps them and is retried at most once a second. Rotation stays the Java operator's.

## Acceptance criteria

- AC-1: every recorded primitive (15 ints as int and vint, 9 vlongs, 12 zlongs and longs, booleans, 4 versions, 309 strings with NUL, two- and three-byte chars and surrogate pairs, optional strings, string arrays, a string map, two task ids) encodes to the Java's bytes and decodes back to its value; a peer version other than 3.10.0 is refused.
- AC-2: the decoder refuses a truncated value at every cut of every recorded encoding, a vint longer than five bytes, a negative length, a string whose bytes are not a valid char sequence, and a value past the writer's limit.
- AC-3: each of 5 recorded `DiscoveryNode`s (IPv4, IPv6, a stream address, every built-in role and a dynamic one, non-ASCII ids, the minimum compatible version) encodes with and without attributes to the Java's bytes and decodes to its fields.
- AC-4: each uncompressed recorded frame (both handshakes each way, five transport handshake responses, requests with a header, features and a parent task, a response with a two-value response header) encodes to the Java's bytes, and every recorded frame, compressed or not, decodes to the same fields and message; a frame Rust compresses decodes to the same message.
- AC-5: a frame header with a bad marker, a length below the fixed header or past the bound, a variable header past 64 KiB or longer than its frame, a frame not the length it announces, or a compressed message whose output passes the bound is refused; a ping (`ES` and length -1) reads as a ping.

- AC-6: each failure a node answers (security 403, rejection, action not found, status 503, each wrapped as `RemoteTransportException`) is written as the Java's bytes, and an error response frame is the Java's frame; every recorded chain (status with metadata and a cause chain, the JDK kinds 1 to 18, a throwable sent as `NotSerializableExceptionWrapper`, none, an unknown subclass) reads to its kind, message, status and cause, and an incomplete one is marked.
- AC-7: over loopback, a call is answered plain and compressed; concurrent calls open at most 4 sockets a peer and an idle socket is reused; an unknown action is `ActionNotFound` wrapped with the node's name, address and action; a security refusal is `Unauthorized` (403) and another failure `Remote` with its status; a full worker queue answers 429; a closed port is `Refused`; a slow answer is `Timeout` and the socket serves the next call; another ephemeral id is `WrongPeer`, and a pooled socket to the old process is closed; a stopped server fails the open socket and then refuses.
- AC-8: a frame past the server's bound is answered 413 and the channel stays open, compressed or not; a frame past the client's bound is `TooLarge` before sending, an answer past it fails that call alone, and an answer more than the pool holds is `Busy`; a frame the pool cannot hold is dropped as 429 at the variable header, the message and the inflation; the transport pool is back to 0 after every call.
- AC-9: a request without a TCP handshake is served (as a Java peer's later channels send them); a ping is echoed; a request at another version is answered 400; a TCP handshake from an unsupported version closes the channel; channels past the cap are closed, and silent or idle ones time out; a garbled or cut frame, or a response on an inbound channel, closes it; a compressed transport handshake is answered compressed.
- AC-10: a live OpenSearch 3.3.0 transport (`Netty4Transport` and `TransportService` at the shell's version id) connects to the Rust server with its 13 channels, gets an echo answered plain and compressed, reads Rust's `ActionNotFound` and status failures as its own exception classes, and refuses the Rust node under another ephemeral id; the Rust client does the same against the Java server.

- AC-11: each recorded forwarded write (4 index requests, 3 index responses, 3 bulk requests and 2 bulk responses, with and without the optional fields, deletions, conditions, failures and non-ASCII text) encodes to the shell's bytes and decodes to its fields, consuming them all; a count is held to 7 bytes an operation and 10 an outcome before anything is sized; a deletion travels and reads without a source, as the shell rebuilds it before digesting.
- AC-12: each of 4 headers the shell's `TransportAuthenticator` signed verifies once for its recipient and is refused a second time and for another recipient, action, request byte or header; a header Rust signs has the shell's five fields and verifies; a timestamp outside the window, or whose expiry overflows, is refused; the previous key verifies for one window after a rotation and not after; a newer generation refreshes the secret once a second and answers 429 meanwhile; a missing header (as on a request carrying only the legacy token) or a malformed one is refused 403; a full replay cache refuses until its nonces expire; a guarded handler answers only signed requests.
- AC-13: every recorded register value (27 cases: bare secrets, objects with every field, defaults, numbers of each kind, unknown fields, escapes, blanks by Java's whitespace, and 12 the shell refuses) reads to the shell's fields or is refused, and 3 values are written as the shell's `toJson`; a read takes the register's generation in one GET; the first node mints a 64-digit key with one conditional create that later reads share, and a node whose create loses reads the winner's; the cache reads once a lease TTL, again when the clock goes back or when asked, retries a failed read at most once a second keeping what it holds, and lets callers go on while one reads; two nodes on one register verify each other's requests and a node on another register is refused.

## Test plan

- AC-1 to AC-5: `crates/transport/tests/` over `fixtures/transport/`, recorded by `WireRecordings.java` with OpenSearch 3.3.0's own classes at the shell's version id. Fails if any byte differs.
- AC-6: `crates/transport/tests/wire/exceptions.rs` over `fixtures/transport/exceptions.txt`, recorded by `WireRecordings.java`.
- AC-7 to AC-9: `crates/transport/tests/wire/{loopback,raw,fake_peer,reader}.rs`: real sockets on loopback, a hand-written peer for answers a node seldom sends, and the frame reader over byte buffers with small pools.
- AC-10: `fixtures/transport/TransportInterop.java` with `crates/transport/examples/interop.rs`, run by hand as `fixtures/transport/README.md` shows (it needs the OpenSearch and Netty jars, which the gate does not have). Every line prints `ok`, or the run fails.
- AC-11 and AC-12: `crates/transport/tests/wire/{forwarding,macs,mac_rotation}.rs` over `fixtures/transport/{forwarding,macs}.txt`, recorded by `ForwardingRecordings.java` with the shell's own classes.
- AC-13: `crates/format/tests/transport_secrets.rs` over `fixtures/transport-secret/`, recorded by `TransportSecretRecordings.java` with the shell's own `TransportSecrets`; `crates/cluster/tests/leases/secret.rs` on the emulator with a simulated clock and exact request counts.
- Mutants over the diff; planted defect: a vint's continuation bit tested on the wrong byte.

## S3 request budget

| Operation | Requests |
|---|---|
| encode or decode anything here | 0 |
| a call, a handshake, a ping | 0 |
| signing or verifying a request | 0 |
| a node's secret, once a lease TTL | 1 GET |
| a failed secret read, retried | at most 1 GET a second, until one succeeds |
| a header naming a generation this node has not seen, signed or not | at most 1 GET a second a node, as the Java's |
| a deployment's first secret, once in its life | 1 GET + 1 conditional create (+1 GET for a node whose create loses) |

## Switch

None: a codec and a transport with nothing to compare against, serving no role yet. The MAC is not a switch either: the shell refuses an unsigned forwarded request, so a Rust node that skipped it could not talk to a Java one. The secret's read is one GET a node a lease TTL, the Java's own rate. Roles wire the actions in when they serve them.

## Risks

- The recordings come from OpenSearch 3.3.0's jars at 3.10.0's version id, not from a build of the shell's 3.10 fork. The encodings recorded are stable across 3.x where the version gates (`V_3_2_0` for a node's stream address) are passed. AC-10 runs a live 3.3.0 transport at the shell's version id; a running shell node is checked when the forwarded actions land.
- The server's memory per channel is a reader thread with a 256 KiB stack, mostly untouched; the compressor never runs on it (an unoptimised build overflowed it, AC-9). At the default cap of 8,192 channels that is 2 GiB of address space, not resident memory.
