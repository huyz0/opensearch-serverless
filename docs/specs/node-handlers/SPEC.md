# One signed registry holds every action a node serves

## Serves

Parity plan R7.6, node assembly, first piece: the transport handlers the role pieces built (the commit push, the newest-commit ask, the tail push and the tail fetch) are registered nowhere. A registry puts each under its action, behind the signature check every internal action needs, and refuses a second handler for an action.

## Change

`oss-node` gains `handlers::Registry` over the transport's `Handlers`: `new(auth)`, `add(action, handler)`, `actions` and `into_handlers`. Each handler is wrapped in `Guarded` with the node's authenticator, so no handler can be registered unsigned. The assembly adds each role's handler under the transport's action constant.

## Acceptance criteria

- AC-1: every handler the registry holds refuses a request that carries no valid signature, before the handler runs.
- AC-2: a signed tail push, sent by a writer's link to a server built from a registry holding `TailPushHandler` under the tail push action, reaches the reader's `TailHeld`.
- AC-3: registering a second handler for an action is refused with the action named, and the first stays.
- AC-4: `actions` lists the registered actions sorted, and `into_handlers` hands over exactly those.

## Test plan

`crates/node/tests/handlers.rs` over the transport rig the tail link tests use: a real server on loopback, signed and unsigned requests.

## S3 request budget

Zero: nothing is read or written.

## Bounds

One entry an action, a handful in all.

## Switch

None: no setting, no measured downside. The registry is for every build; the handlers a role adds exist with its feature.

## Risks

None known: the transport's own bounds apply to each handler.
