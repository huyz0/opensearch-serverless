# A writer restores a shard's published commit into its directory

## Serves

R8.1, takeover of a shard that has a history: a node that wins the head of a shard with a published commit must open its engine on that commit before it replays a predecessor's log, so the shard's documents and sequence numbers carry on. [Writer takeover](../writer-takeover/SPEC.md) assumes the engine was opened from the commit; this is how it is.

## Change

`oss-writer` gains `takeover::restore`:

- `restore(store, base, shard, dir, buf)` reads the shard's register once (a GET like any register read) and, when it names a commit, copies every file of it into `dir`, one ranged GET a blob (the span of the blob's files, streamed through `buf`), and returns the commit's term and generation. A register that names no commit restores nothing and answers `None`.
- A blob shorter than its files need and a register that does not parse are errors; `dir` is then left for the caller to drop.

## Acceptance criteria

- AC-1: after `restore`, a `Shard` opened over `dir` holds the published documents and continues its sequence numbers after the commit's `max_seq_no`.
- AC-2: every file of the commit is in `dir` with the published bytes.
- AC-3: `restore` makes one register read and one GET a blob the commit's files are in, however many files; 0 LIST, 0 PUT.
- AC-4: a shard with no register restores nothing, answers `None`, and makes no GET.
- AC-5: a blob that ends before one of its files does is an error; a file published as a blob of its own with no recorded length is read whole.

## Test plan

`crates/writer/tests/it/restore.rs` over the emulator and a real `Shard`, each seen failing first against a `restore` that does nothing.

## S3 request budget

One register read and one ranged GET a segment blob of the commit. Nothing scales with documents or elapsed time. LIST 0.

## Risks

- The whole commit is held in the directory, in memory, as the writer already holds the files it writes; a very large shard is bounded by the indexing memory story, not by this.
