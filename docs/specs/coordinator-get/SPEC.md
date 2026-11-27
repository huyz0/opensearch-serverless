# The coordinator decides where a get is served and whether its version holds

## Serves

Parity plan R7.3, document APIs, first piece: the decisions in the Java shell's `DocumentReads.doGet` and `readAcceptedGet` (ops package, shell 429b4cbc) that need no node: where a get is served, and whether the version a caller pinned still holds. The reads themselves, the forward over the transport and the source filter are the node assembly's and their own pieces.

## Change

`oss-coordinator` gains `docs::get`: `plan_get`, which turns a placement (from `Router::place`) into a `GetPlan`; and `version_conflict`, which says whether a found document's version conflicts with the one a get pinned and in what words.

## Acceptance criteria

- AC-1: a placement this node holds, whose owner is nobody or this node, is read here (`GetPlan::Local`); the manifest lookup is not made.
- AC-2: a placement held here whose head names another node is not read here (the retained writer may lag the owner): it is forwarded to the owner.
- AC-3: with no owner and nothing ever published, the document has never existed (`GetPlan::Absent`); with no owner and a commit published, it is read from the commit (`GetPlan::Reader`). A manifest lookup that fails is the lookup's refusal.
- AC-4: an owner that is another node is forwarded to (`GetPlan::Forward`) with that owner's id.
- AC-5: an owner that is this node, with no shard held here, is refused with `Router::not_here`'s words ("this node is acquiring shard N of i; retry") and doubts ownership.
- AC-6: a pinned version of "any" (-3) never conflicts; "deleted" (-4) always conflicts with a found document; any other conflicts exactly when it differs from the document's; the message is "current version [c] is different than the one provided [e]", and for a missing document "document does not exist (expected version [e])".

## Test plan

`crates/coordinator/tests/it/get.rs`: an in-memory plane, a counting manifest lookup, the words asserted exactly.

## S3 request budget

At most the one manifest read the shell made for an unowned shard; none when this node holds the shard or another node owns it.

## Bounds

None to hold: one decision per get.

## Switch

None: the shell's get has no setting and no measured downside.

## Risks

The shell checks the version only for a found document; a caller that asks about a missing one must test for it first.
