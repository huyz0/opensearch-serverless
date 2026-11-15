# Filesystem store

## Serves

The plan's first storage-backend task: `serverless.store.type = fs` beside `s3`, so a single-host node and the shell's request-budget tests run on a directory, and a directory the Java shell's `FsBlobStore` wrote is readable here (invariant I-1: every store call is metered).

## Change

- `oss-blob` gains `FsStore`, a `BlobStore` over a directory, ported from the shell's `FsBlobContainer` and `FsBlobStore` (huyz0/OpenSearch `server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobContainer.java@3920dc36`), and the store settings `serverless.store.type` (`fs` or `s3`) and `serverless.store.path`, parsed as `ObjectStores.create` does (`serverless/shell/src/main/java/org/opensearch/serverless/store/ObjectStores.java@3920dc36`).
- Key layout is the Java's: the key `a/b/c` is the file `<root>/a/b/c`; each `/`-separated part is a path component of at most 255 bytes.
- `put` stages the body in a temporary file, syncs it, and renames it over the key; `create_if_absent` stages it the same way and publishes it with a hard link, which fails when the name is taken. A key is never seen part-written (the Java's `createWhole` reasoning). Temporary files live in the reserved directory `.opensearch-register-locks#` beside the key, which the Java's listings and blob names already exclude, so neither side ever lists one.
- `list_prefix` walks the directories under the prefix and returns one page in byte order of the whole key, holding at most `max_keys + 1` keys, as S3's flat listing would.
- Every call records one attempt of its S3 kind in the store's `Meter`, as the Java's `MeteredBlobStore` counts its filesystem store, so the request-budget tests mean the same on fs.
- Not in this change: registers and generation-checked swaps (the next change, on all three backends), wiring a store into a running node, cleaning up temporary files a crash left behind, batch delete.

## Acceptance criteria

- AC-1: on success each call records exactly one attempt of its kind and nothing else: GET `Get`, ranged GET `GetRange`, PUT and conditional create `Put`, HEAD `Head`, DELETE `Delete`, LIST page `List`; a failed call records its one attempt and one error.
- AC-2: the key `a/b/c` is stored as the file `<root>/a/b/c` holding exactly the body, and a store opened on `fixtures/shell-blobs` (written by the Java shell's `FsBlobStore`) reads each fixture blob byte for byte and lists its keys.
- AC-3: a ranged GET returns exactly the bytes asked for, truncated at the end; a range at or past the end is `InvalidRange`; a missing key, or a key that names a directory, is `NotFound`; bodies stream through the caller's buffer, no chunk larger than it.
- AC-4: `put` replaces the object whole; a body shorter or longer than its declared length fails with `InvalidArgument` and leaves the previous object and no temporary file behind.
- AC-5: `create_if_absent` on a taken key is `AlreadyExists` and leaves the object unchanged; of 8 threads racing to create one key, exactly one gets `Created` and the stored body is that thread's.
- AC-6: no reader ever sees a key part-written: while 4 threads replace a 1 MiB object with bodies of one repeated byte each, every concurrent GET returns 1 MiB of a single byte value.
- AC-7: HEAD of a missing key, or of a directory, is `None`; DELETE of a missing key succeeds; after DELETE, GET is `NotFound`.
- AC-8: a LIST page visits at most `min(max_keys, 1000)` keys starting with the prefix, in byte order of the whole key across nested directories (`a/b-c` before `a/b/c`), after `start_after`, with sizes, and reports truncation; 2,500 keys are listed in exactly 3 pages; temporary files and the reserved directory are never listed, and the walk never enters a directory below the 64-component depth a key can reach.
- AC-9: an empty key, a key over 1,024 bytes, an empty, `.` or `..` component, a component over 255 bytes or containing `\` or NUL, a key of more than 64 components, a key inside the reserved directory, an empty buffer, a zero-length range and `max_keys` of 0 fail with `InvalidArgument` before any attempt is recorded.
- AC-10: settings `serverless.store.type` absent or `fs` with `serverless.store.path` give an fs store; `fs` without a path, and any type other than `fs` or `s3`, are refused with the Java's messages; `s3` without `serverless.store.bucket` is refused.
- AC-11: a 16 MiB object is written and read through one 64 KiB buffer with peak heap under 256 KiB above the start, counted by the test allocator.
- AC-12: every request-path allocation states its bound (`scripts/check-alloc.py`) and the one LIST implementation carries `// LIST:` (`scripts/check-s3-access.py`).

## Test plan

- AC-1 to AC-9: `crates/blob/tests/fs/` against a fresh temporary directory, asserting meter counts; each seen failing against a planted defect: a body written in place instead of staged (AC-6), a hard link replaced by an exists-then-write check (AC-5), directory entries compared by name rather than by whole key (AC-8), temporary files staged next to the key (AC-8), a length check skipped (AC-4).
- AC-2: the same suite opens `fixtures/shell-blobs` read-only and compares against `std::fs` reads of the same files.
- AC-10: unit tests in `src/settings.rs`.
- AC-11: `tests/fs_streaming_peak.rs`, its own binary because the counting allocator is process-wide.
- AC-12: the gate.

## S3 request budget

The same table as the S3 client's, one attempt per call and no retries, since a local filesystem has no transient failures to retry: `get`, `head` 1 GET-class; `put`, `create_if_absent` 1 PUT-class; `delete` 1 free; `list_prefix` 1 LIST. Depth 1 each. Unchanged for every caller.

## Switch

None: a new store nothing uses yet; `serverless.store.type` defaults to `fs` as the Java's does, and only the settings parser reads it.

## Risks

- A shared or network filesystem gives no atomic hard link or rename across hosts: fs is single host only, as the Java documents. Owner's decision (plan, open question 2).
- A key that is both an object and a prefix (`a` and `a/b`) cannot exist on a filesystem; the second write fails with `InvalidArgument`. The Java has the same limit; shell keys never do it.
- Keys are capped at 64 components here, which the Java does not cap: a LIST holds one open directory per level, so depth bounds its descriptors. The shell keys in `fixtures/` have at most four.
- A new key's new directories are synced with it; the Java syncs only the key's own directory.
- A crash between staging and publishing leaves a temporary file in the reserved directory, invisible to both listings; nothing reclaims it yet.
- Windows: renaming over a file another handle has open needs delete sharing, which Rust's `File::open` grants; directory sync is a no-op there, as in the Java's `IOUtils.fsync`.
