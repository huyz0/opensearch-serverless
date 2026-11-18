# Tail wire: the tail fetch and tail push messages

## Serves

Parity plan R7.6, the tail overlay's wire half, and [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md): Rust and Java nodes talk to each other directly. Builds on [writer-tail-keep](../writer-tail-keep/SPEC.md), whose kept records these messages carry.

## Change

`oss-transport` gains `actions::tail` (`crates/transport/src/actions/tail.rs`): `FetchRequest` and `FetchResponse` (`internal:serverless/tail/fetch`, the shell's `TailFetchRequest` and `TailFetchResponse`), `PushRequest` and `PushResponse` (`internal:serverless/tail/push`, `TailPushRequest` and `TailPushResponse`), with `Record` for the shell's `TailRecords` form (a record's id, a deletion flag, its source, `seq_no`, `primary_term`, `version`, an optional routing; a deletion's source is always written empty). Each reads and writes as the Java's `StreamInput` constructor and `writeTo` do. A decoder sizes a list only by the bytes left. `fixtures/transport/TailRecordings.java` records the rows with the shell's own classes.

Not changed, by design and listed: the fetch handler, the push sender and the reader's receiver, the subscriptions and grants, and the request MAC, which `Guarded` already applies to any action.

## Acceptance criteria

- AC-1: every recorded fetch request (a new subscription, a renewal, a check, with and without a parent task, non-ASCII names) writes the bytes the Java wrote and reads back to the same value, and every shorter cut of those bytes is refused.
- AC-2: every recorded fetch answer (no records, indexed and deleted records, with and without routing, an unsubscribed answer, a claim of `i64::MAX`) does the same.
- AC-3: every recorded tail push request (one shard, several, an empty record list) and answer (none unwanted, some) does the same.
- AC-4: a message that claims more records, shards or unwanted shards than its bytes can hold is refused before anything is sized for them.
- AC-5: the action names are the shell's, and the sentinel values (`NEW` -1, `CHECK` -2, `NONE` -1) are the shell's.

## Test plan

`crates/transport/tests/wire/tail.rs` over `fixtures/transport/tail.txt`, recorded from the shell's classes by `TailRecordings.java`.

## S3 request budget

None: nothing here touches the store.

## Switch

None: the messages are inert until a handler and a sender use them.

## Risks

- The recordings come from the shell's classes compiled against the 3.10 snapshot's jars: a change of the Java's `writeTo` shows when the recorder is re-run.
