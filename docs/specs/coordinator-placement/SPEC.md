# The coordinator places a document and decides what a failed forward means

## Serves

Parity plan R7.1, request routing, second piece: the Java shell's `RequestRouting` (ops package, shell 429b4cbc) and `ForwardFailure`, `NotHereException`, `Placement`. The hash that picks a shard (`oss-cluster`'s `routing`), the owner lookup (shard heads) and the owner hint are the caller's, behind a trait; the node assembly adapts them.

## Change

`oss-coordinator` gains `route::place`: `Router::place(index, id, routing)` (the descriptor and owner a request has looked up once each, the shard, whether this node holds it, and its owner), `Router::after_forward_failure(index, stale, failure, may_retry)` (what to do after a forward failed: forward to a new owner, or refuse with the shell's words), and `not_here(index, placement)` (why this node cannot serve a placement). `Failure` carries the shell's seven classes of forward failure.

## Acceptance criteria

- AC-1: a system index is refused before its descriptor is read when the caller refuses system indices; a missing index is refused as `no such index: n`.
- AC-2: a placement is the shard the hash gives, the descriptor's uuid, whether this node holds exactly that shard, and its owner: the node's hint if it has one, else the head's owner; the descriptor is read once for any number of documents of one index, and the owner once for each shard (a hint or a head read, never both).
- AC-3: after any failure the hint and the request's memo for that shard are forgotten first.
- AC-4: a lost condition (`Conflict`) is rethrown as itself; an owner's own client-side answer (`Rejected` or `Remote` under status 500) is rethrown as it is; a server-side one is refused as not here, retryable, worded as the forward that failed, naming the stale owner.
- AC-5: a deadline or a dropped connection is never retried: ownership is doubted and the refusal tells the caller the write may have been applied there and to read before retrying.
- AC-6: a refusal by a non-owner or an unreachable owner, with retry allowed, reads the head once: a new owner that is neither the stale one nor this node is forwarded to (a placement with no local shard, the same uuid); no owner is refused as "no node currently owns shard N of i; activate it before writing", not retryable; this node as the owner is refused as "this node is acquiring shard N of i; retry"; the same owner is refused as the forward that failed, or in the unreachable-owner's own words. Ownership is doubted on every refusal path. With retry not allowed, no head is read.
- AC-7: `not_here` words the three cases as the shell does: no owner (not retryable, owner none), this node (acquiring, retryable) and another node (owned by it, retryable), doubting ownership for the first two.

## Test plan

`crates/coordinator/tests/place.rs` over an in-memory plane that counts descriptor, hint and head reads and records doubts and forgets; the refusals' wording asserted exactly.

## S3 request budget

One descriptor read a distinct index and one head read a distinct shard in a request, only when no hint stands; a failed forward costs at most one more head read. Requests scale with the distinct indices and shards in the request, never its documents.

## Bounds

The memos hold one entry for each distinct index and shard a request touches.

## Switch

None: the shell's routing has no setting and no measured downside.

## Risks

The classification of a transport error into a `Failure` belongs to the forwarding piece that holds the transport client.
