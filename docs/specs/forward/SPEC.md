# Forwarding a bulk to a shard's owner, and where early revoke is called

## Serves

Parity plan R7.1 (routing) and the carry-over from the transport spec: the one call site of `EarlyRevoke::on_refused` (`serverless.lease.early_revoke`, off by default; [transport spec](../transport/SPEC.md), AC-14). Invariants: S3 requests are the bill (a refusal off the setting reads nothing); fencing is by conditional swap, never a lock ([principles](../../principles.md#write-path)).

## Change

- `oss-node` gains `forward::BulkForwarder` behind the `writer` feature (`crates/node/src/forward.rs`), ported from the send side of the shell's forwarded bulk. It finds the owner's unrevoked lease in the leases the node already holds, signs the request as every forwarded request is, calls the owner's `internal:serverless/document/bulk` and reads the `BulkResponse`.
- When the call is refused at connect (`CallError::Refused`, nothing listens), the forwarder calls `EarlyRevoke::on_refused` with the lease's node id, ephemeral id and address and the time of the refusal. A `Revoked` or `Gone` verdict answers `ForwardError::Gone`: the owner is to be looked up again and the normal takeover applies. Off or declined answers the refusal as a failed call.
- Every other failure (timeout, reset, a remote exception, an unusable lease) is a failed call and never reaches early revoke.
- Not changed: turning a group into a `BulkRequest` and the response into item results (the `Routes` implementation), the coordinator's own forwarding of searches.

## Acceptance criteria

- AC-1: a signed bulk reaches the owner's guarded handler and its `BulkResponse` comes back.
- AC-2: with early revoke off, a refused connect is a failed call and reads nothing from the store.
- AC-3: with it on, a refused connect to a settled lease whose confirming connect is refused revokes the lease with one read and one swap, and the forward answers `Gone`.
- AC-4: a refusal early revoke declines (a lease renewed within the settle time) is a failed call and writes nothing.
- AC-5: a timeout or a remote exception never calls early revoke, and a node with no unrevoked lease (absent or revoked), or an address that does not parse, fails without a call.
- AC-6: a bulk larger than `MAX_FORWARD_BYTES` is refused as `TooLarge` without a call, so it is not retried as if the owner were unknown.

## Test plan

`crates/node/tests/roles/forward.rs`: a real socket for AC-1 and AC-5's remote exception, a closed local port for the refusals, an emulator-backed `Membership` on a manual clock and a counting `ConnectProbe` for AC-2 to AC-4; each test run red before the code.

## S3 request budget

None on a forward that succeeds. A refusal with the setting off: zero. With it on: one lease read and at most one swap per refused process per lifetime (the negative cache in `EarlyRevoke`).

## Bounds

One request message, at most `MAX_FORWARD_BYTES` (32 MiB; a bulk's own limits keep a request far below it), one signature and one header a call. Nothing is kept between calls. Reserving that buffer from the transport pool is the `Routes` implementation's, which knows the bulk's size before it builds the request.
