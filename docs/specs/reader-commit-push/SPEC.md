# Reader: take a commit its writer pushed

## Serves

Parity plan R6.5 (the reader takes pushes), first slice, and the read flow in [architecture](../../architecture.md): a reader opens the latest commit it was told about, and readers never poll for it. Builds on [reader-open-commit](../reader-open-commit/SPEC.md).

## Change

`oss-reader` gains `commit_push` (`crates/reader/src/commit_push/`), ported from the shell's `CommitPush` receive side, `NewestCommits.newerThan` and `CommitManifest.stagedAfter`. It is sans I/O: the transport that carries a push (a later slice) calls it.

- **Mailbox**: `PushedCommits` keeps, per shard, the newest commit its writer pushed, as the commit register's value (the bytes the writer wrote), until a reader open takes it once. A push is validated by parsing it. A commit older than the ten-second keep time is not handed out. The mailbox is bounded in shards (4,096) and in bytes (a budget the node gives it); a push that does not fit is refused and the reader reads the register, as before pushes existed.
- **Which is newer**: `staged_after(a, b)`: both are staged by one writer in one term, `a` later, as `CommitManifest.stagedAfter`. A pushed commit replaces a held one only when it is not older by this rule. `newer_than(register, newest)` gives `newest` when it is staged after the register's commit, else the register's.
- **Open from a push**: `ShardFiles::open_pushed_with_meta` and `open_shard_pushed` open the commit a writer pushed with the descriptor read alone: one GET, and no commit register read.

Not changed, by design and listed: the transport action that carries a push, the answer a reader gives (whether it has the shard open), the blocks a writer sends along, the tail records, and moving an already open reader onto a pushed commit. Each is a later slice of R6.5.

## Acceptance criteria

- AC-1: opening from a pushed commit register value costs exactly one GET (the descriptor), no LIST, HEAD or PUT, and finds the same files as the two-GET open of that commit.
- AC-2: a push is kept until taken, taken once, and the next take finds nothing.
- AC-3: a pushed commit older than the keep time is not handed out and is dropped; one exactly as old as the keep time still is.
- AC-4: a push replaces the held one for its shard unless the held one is staged after it (one writer, one term, a later stage); one from another writer or term, or a later stage, replaces it.
- AC-5: a push that is not a commit register value is refused and holds nothing.
- AC-6: past the shard bound or the byte bound a push is refused and the held ones stay; bytes held come back when a push is taken or dropped.
- AC-7: `staged_after` is true only when both stage sequences are known, the term and the writer are the same, and the first is later; `newer_than` gives the register for an unstaged register, an unrelated newest, or an older newest.

## Test plan

`crates/reader/tests/it/push.rs` (AC-1, the fixtures through the emulator with exact request counts); `crates/reader/tests/commit_push.rs` (AC-2 to AC-7, pure, with a manual clock).

## S3 request budget

Open from a push: 1 GET (descriptor) then the engine's ranged reads, against 2 GETs from the register. LIST 0, HEAD 0, PUT 0. The mailbox makes no request.

## Switch

None: the mailbox is empty until a transport fills it, and the open from the register is unchanged. Whether a node pushes is the writer's setting, in a later slice.

## Risks

- A pushed commit that the register has since moved past is opened: the keep time bounds it, and the reader's register look moves it, as the shell does.
- The bytes held are a heap cost: the byte bound is the node's, charged to its memory budget when the transport wires the mailbox.
