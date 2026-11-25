# Commit push to the readers of another build

## Serves

Parity plan R15, the Java-retire gate. With the tail overlay on (the default) a Rust node does not push each commit it publishes: its own readers get freshness from the overlay, and a push to them was measured to make readers reopen cold. A Java reader cannot take the overlay. Its freshness check asks the writer how far it has answered as refreshed and refuses the shard (`shard_behind_log_exception`, a partial answer) while its open commit is below that, until a commit reaches it. After a Rust owner was killed and a Rust node took its shards, Java readers refused them for minutes ([mixed-fleet-wide-soak](../mixed-fleet-wide-soak/VERIFIED.md)).

## Change

A Rust writer that does not push every commit still pushes every commit to the readers its placement ranks first that are not of this build (a lease whose `version` does not start with `oss-rust/`). `PushTargets` gains `other_build_readers`; `Published` says whether only those hear of a commit; `CommitPush` queues a routine commit for them when it would not queue it for anyone, and a takeover's first commit still goes to every ranked reader. With `OSS_COMMIT_PUSH` on, every commit goes to every ranked reader, as before.

## Acceptance criteria

- AC-1: `PlacementTargets::other_build_readers` returns the ranked readers other than this node whose lease does not name this build, and none that does; a lease with no version is another build's.
- AC-2: with the push to every reader off, a routine commit is sent to the readers of another build only and a takeover's first commit to the ranked readers.
- AC-3: in the wide mixed-fleet soak no Java reader refuses a shard after a Rust owner is killed.

## S3 request budget

None: a push is a transport call to a node, not a request to the store, and a fleet of one build has no such reader, so it sends nothing new.

## Switch

None. A fleet of one build is unchanged; in a mixed fleet it sends each commit to at most the ranked readers (two by default) of the other build, a transport message the size of a commit register. A Java reader that is not pushed to stays behind its Rust writer with no way to catch up, so there is no case where it wins by being left out.
