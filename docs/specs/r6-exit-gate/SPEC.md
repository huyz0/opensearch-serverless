# R6 exit gate: Java and Rust readers open each other's commits

## Serves

Parity plan R6 exit gate, and the risk in [segment-publish](../segment-publish/SPEC.md): a blob or manifest the Java cannot open. Invariant I-6 is green with [#99](../../invariants.md).

## Change

No product code. `fixtures/exit-gate` holds a commit the Rust writer published, the Java check that opens it with the shell's own classes (`CommitManifest.fromStream`, `FileLocation.parse`) and a Lucene reader through the soft-deletes wrapper, and what the Java printed. `crates/node/examples/write_commit.rs` writes the commit; `crates/node/tests/exit_gate.rs` opens the same bytes with the Rust reader.

## Acceptance criteria

- AC-1: the shell's classes parse the Rust-written commit register's value and every file location, each location's length equals the manifest's, and a Lucene reader opens the files and counts the live and total documents (recorded in `java.txt`).
- AC-2: the Rust reader opens the same bytes and counts the same live and total documents, the same term, writer and number of files.
- AC-3: the Rust reader opens what the Java shell wrote (`fixtures/compat-blobs`, `fixtures/shell-blobs`) and answers its search fixtures as OpenSearch did.
- AC-4: the bulk, document and mapping fixtures recorded from stock OpenSearch replay through the Rust writer's own code, and its segments index as the Java indexes them.

## Test plan

AC-1 is run by hand (`fixtures/exit-gate/README.md`), because a Java build is not in CI, and its output is the committed `java.txt`; AC-2 is `crates/node/tests/exit_gate.rs`; AC-3 and AC-4 are the existing replays.

## S3 request budget

None: no code that makes a request changes.

## Switch

None.

## Risks

- `java.txt` is Java's answer for the committed bytes only. A change to the writer's segment layout needs the example and the Java check re-run together, and the README says how.
