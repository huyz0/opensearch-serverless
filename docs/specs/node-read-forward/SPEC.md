# A document get forwarded to the shard's owner

## Serves

R7 cluster-wide reads: a realtime get of a shard another node holds must reach its owner, because only the owner holds writes that are acknowledged and not yet published ([transport get message](../transport-get/SPEC.md), [forward](../forward/SPEC.md)). This is the handler and the sender; the REST route uses them separately.

## Change

`oss-node` (feature `writer`) gains `read_forward`:

- `GetHandler`, a transport handler for `internal:serverless/document/get`: it resolves the index by name, checks the incarnation when the request names one, reads the document from the shard this node holds with a realtime get, and answers a `GetResponse` naming this node. An unknown index, a stale incarnation, a shard not held here and a malformed request each answer a failure, never a guess.
- `GetForwarder`, the send side: it finds the owner in the leases this node already holds, signs the request, calls the owner and decodes the answer, with the errors of the forwarded bulk (`ForwardError`).

## Acceptance criteria

- AC-1: a forwarded get returns an acknowledged write that no refresh has published, with its source, version, sequence number and primary term, answered by the owner.
- AC-2: a forwarded get of an id that does not exist answers `found: false` with no source.
- AC-3: an unknown index, a stale incarnation uuid, a shard not held by the owner and a negative shard each fail with the owner's remote failure.
- AC-4: a get for a node no unrevoked lease names sends nothing.

## Test plan

`crates/node/tests/read_forward.rs`: a real owner (a writer role holding a shard, written through its REST door) behind a real signed transport server, called by a `GetForwarder` over a socket. AC-1 was observed failing with the handler's realtime flag turned off.

## S3 request budget

None: no blob request on either side; the owner's lease comes from leases already held.

## Risks

- The sender copies the bulk forwarder's sign-and-call steps; they should share one function when the two files are no longer being changed in parallel.
- A forwarded get answers the document's routing as absent: the engine does not return the stored routing value yet.
