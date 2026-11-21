# The writer waits for its readers before answering a refreshing write

## Serves

Parity plan R9.3 (refresh semantics), writer half: the `awaitSearchNodes` of the shell's `TailPush` and its `RefreshVisibility` modes (`serving`, `everywhere`). Builds on [writer-tail-push](../writer-tail-push/SPEC.md) and [writer-tail-fetch](../writer-tail-fetch/SPEC.md), which left "waiting for them" and the lapsing grants to this piece. Not here: the reader's grant check and its `CHECK` asks, the write route calling the wait, the `serverless.refresh_visibility` mode and `forced_refresh` in the answer.

## Change

`oss-node`:

- `TailServe` gains the waiting state: `open_wait(uuid, shard, target)` (a wait for every subscribed node still behind `target`, none when none is), `wait(id, timeout)` (whether every node was reached, ended either way), `drop_node(uuid, shard, node, sent, may_still_serve, lapses)`, `lapse_deadline` and `pause_until`.
  - A push's answer settles the waits it completes. A node dropped with `may_still_serve` marks the wait not reached; one dropped with `lapses` leaves its grant to be waited out. `unsubscribe_all`, `stop` and `released` end a shard's waits as not reached.
  - `lapse_deadline` is the later of 600 ms after this node began keeping the shard (an earlier writer's grant may still run) and the grant of every node dropped with `lapses`.
- `TailPush` gains `await_search_nodes(uuid, shard, timeout, grants)` (module `tail_await`): claims the shard, begins its keep when `grants`, opens a wait for the position acknowledged so far, pushes what is queued at once, and waits at most `timeout` (`WAIT`, 500 ms, is the shell's). With `grants` it then waits on the clock until `lapse_deadline`. It is true only when every node was reached.
- `flush` drops a node with `may_still_serve` and `lapses` when a push is lost (the shell's send failure; this link cannot tell a missing lease apart, and the conservative reading is the safe one) and with neither when the node answers that it has no overlay for the shard.
- The wait blocks: its caller runs it where a write may wait. Nothing calls it yet.

## Acceptance criteria

- AC-1: a shard nothing is kept for waits for nothing and sends nothing; without `grants` it never pauses.
- AC-2: a node behind the position is sent the push by the wait itself, and the wait ends true when every node answered; with nothing behind it sends nothing.
- AC-3: a lost push ends the wait false and unsubscribes the node; an answer saying the node has no overlay for the shard ends it true.
- AC-4: a wait times out false after its timeout, and ends false at once when the shard is let go.
- AC-5: with `grants`, a shard this node began keeping now waits until 600 ms after it began; once past it waits no more.
- AC-6: with `grants`, a node dropped for a lost push is waited out until its grant runs out; one that answered "no overlay" leaves no grant to wait out.

## Test plan

`crates/node/tests/tail_await.rs`, with a clock that moves only when paused on and a link that fails or refuses as scripted. The tests were written against an API that did not exist (red by compilation).

## S3 request budget

None: memory and the push link only. LIST 0, HEAD 0, PUT 0.

## Risks

- A refreshing write now blocks up to 500 ms plus a grant (600 ms) once a node was lost; nothing calls it until the write route does, behind the visibility mode's switch.
