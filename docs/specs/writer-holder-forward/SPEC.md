# A write for a shard another live node holds goes to that node

## Serves

Issue #219, found by the multi-node cluster suite, and the placement rule in [cluster routing](../node-cluster-routing/SPEC.md): ownership is rendezvous placement over the members a node sees, but the shard head, once won, stays with the node that won it. When placement later names another node (a node joined, or two views differed while the cluster settled), that node's open found the head held and every write answered 503 `[index][shard] is owned by node [X]` for as long as X lived.

## Change

`HeadOpener` tells a refusal for a head held by another live node apart from every other refusal: `OpenRefused` carries the holder's node id (`ShardHead.owner_node_id`) beside the reason. `ShardOpener::open` returns it.

`WriterRole` (crates/node, `writer/redirect.rs`):

- `apply_group` on a shard not held here first checks the redirect table (shard to holder id); with an entry it forwards the group to that node through `Routes::forward`. With none it asks the opener; a refusal naming a holder records the redirect and forwards the group.
- `owner` answers `Owner::Node(holder)` for a shard with a redirect, so a bulk forwards without entering the opener (a refresh follows `owner`, so it asks the holder).
- A forward that answers any item 5xx drops the redirect: the next group reads the head again and either finds the holder alive, takes the shard (its claim expired or its lease was revoked), or forwards again.
- Opening a shard here drops its redirect.

The node a forward reaches applies it through `apply_group` like any group, so a forward that reaches a node that does not hold the shard either finds a redirect or forwards once more to the holder it reads from the head. The head names one holder at a time, so the chain ends at the node that holds the shard.

Not changed, by design and listed: the head (no new field, no new CAS), the takeover protocol and its terms and fencing (a holder is replaced only by `acquire`, as before), the transport and the forwarded bulk's wire form. A handoff, in which the holder releases a shard placement no longer gives it so that the placement owner takes it over at the next term, is not part of this: it costs a flush, a publish and a head write per shard moved, and is its own change.

## Acceptance criteria

- AC-1: a write for a shard whose head a live node holds is not opened here: the head still names the holder and its term, the group goes to the holder once, and its items answer what the holder answered.
- AC-2: a second write for that shard forwards to the holder without a blob request and without asking the opener again.
- AC-3: when the forward fails (5xx) the redirect is dropped: the next write reads the head again.
- AC-4: a head that names this node, or no live node, is opened here as before; a shard this node holds is applied here and never forwarded.

## Test plan

`crates/node/tests/roles/writer_opener.rs` over the emulator with the real head store and membership, and a `Routes` that records forwards. Each test seen failing first against the opener's old refusal.

## S3 request budget

First write for such a shard: the head's acquire (1 GET), as the failing open made before. Later writes: none. Steady state: no blob request; the cost is one transport hop to the holder. A holder that stops answering: 1 head GET per group until the shard is taken. LIST 0. Nothing scales with documents, indexes or elapsed time.

## Switch

None: the change replaces a permanent failure. The one measured cost, a transport hop for a shard whose holder is not its placement owner, is paid only until a handoff moves the shard.

## Risks

- A forward chain that loops: the head names one holder, and a node that holds the shard applies it locally, so a chain ends there; the forward timeout bounds a head that moves while a group is in flight.
- A stale redirect: a 5xx from the holder drops it, and a holder that no longer holds the shard answers by forwarding or failing, never by applying it.
