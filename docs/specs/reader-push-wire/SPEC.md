# Reader push wire: the commit push and newest-commit messages

## Serves

Parity plan R6.5 (the reader takes pushes), second slice, and [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md): Rust and Java nodes talk to each other directly. Builds on [reader-commit-push](../reader-commit-push/SPEC.md), whose mailbox these messages fill.

## Change

`oss-transport` gains `actions::push` (`crates/transport/src/actions/push.rs`): `PushRequest` and `PushResponse` (`internal:serverless/commit/push`, the shell's `CommitPushRequest` and `CommitPushResponse`), `NewestRequest` and `NewestResponse` (`internal:serverless/commit/newest`, `NewestCommitRequest` and `NewestCommitResponse`). Each reads and writes as the Java's `StreamInput` constructor and `writeTo` do. A commit travels as the register value the writer wrote and is kept as read, because the request MAC digests the receiver's re-serialization. A decoder sizes a list only by the bytes left. `fixtures/transport/PushRecordings.java` records the rows with the shell's own classes.

Not changed, by design and listed: the handlers that receive these (the reader's server side) and the sender (the writer's push), which are the next slices of R6.5; the request MAC, which `Guarded` already applies to any action.

## Acceptance criteria

- AC-1: every recorded push request (with and without blocks, with and without a writer and a parent task, non-ASCII names) writes the bytes the Java wrote and reads back to the same value, and every shorter cut of those bytes is refused.
- AC-2: every recorded push answer, newest-commit request and newest-commit answer (a commit, or none) does the same.
- AC-3: an answer of no commit and an answer of a commit of no bytes are told apart.
- AC-4: a push that claims more blocks than its bytes can hold is refused before anything is sized for them.
- AC-5: the action names are the shell's.

## Test plan

`crates/transport/tests/wire/push.rs` over `fixtures/transport/push.txt`, recorded from the shell's classes by `PushRecordings.java`.

## S3 request budget

None: nothing here touches the store.

## Switch

None: the messages are inert until a handler and a sender use them.

## Risks

- The recordings come from the shell's classes compiled against the 3.10 snapshot's jars: a change of the Java's `writeTo` shows when the recorder is re-run.
