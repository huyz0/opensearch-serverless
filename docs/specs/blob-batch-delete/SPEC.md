# Blob batch delete

## Serves

Invariant I-1 (every request through `oss-blob`, metered). It is the follow-up the node-log store, the blob store, the tombstone sweep and ADR [0012](../../adr/0012-index-deletes-and-reclaim.md) each name: deleting n objects took n round trips in sequence.

## Change

- `BlobStore` gains `delete_many(&[&str])`, and `oss-blob` gains `MAX_DELETE_KEYS` (1,000, S3's limit). The trait's default checks every key, then deletes one key at a time. The filesystem store and test wrappers keep that default.
- `S3Store` sends S3's `DeleteObjects`, `POST /bucket?delete`, for each 1,000 keys, one after another:
  - the body is built whole, with each key XML-escaped, in quiet mode;
  - the request carries `Content-MD5`, which S3 requires for this call. `md-5` is pinned `=0.11.0`, RustCrypto, the same family as `sha2`;
  - the answer is scanned as it streams. The first `<Error>` fails the call: `AccessDenied` is `Denied`, `SlowDown` is `Throttled`, and anything else is `Unavailable`. Deleting is idempotent, so a failed batch is retried whole under the client's policy.
- The emulator deletes each 1,000 keys in one attempt. Its S3 front end serves `POST ?delete`. It refuses a missing `Content-MD5` (400 `InvalidRequest`) and a wrong one (400 `BadDigest`), as S3 does. It also refuses a body naming no key, over 1,000 keys, or over 8 MiB (400 `MalformedXML`).
- `Rounds` counts each batch as one round.
- `NodeLogStore::delete_range` deletes 1,000 slots a request. Before this change it sent one DELETE a slot.
- Not in this change: callers other than the node log. The tombstone sweep and the index purge still delete one key a call, and each moves when its own change wants it.

## Acceptance criteria

- AC-1: 2,502 keys, two of them missing, are deleted in exactly 3 `Delete` requests on the emulator. 1, 1,000 and 1,001 keys take 1, 1 and 2 requests. Over HTTP, 2,505 keys take 3 `POST ?delete` requests on both the client's meter and the server's. Every key named is gone, and a key not named is kept.
- AC-2: an empty key or one over 1,024 bytes anywhere in the list fails with `InvalidArgument` before any request is sent or recorded. An empty list sends nothing.
- AC-3: a failed batch leaves the keys of later batches in place, and deleting again finishes the job. Over HTTP a 503 is retried, with each attempt recorded and each failure counted once. A 403 is `Denied` after one attempt.
- AC-4: keys holding `&`, `<`, `>`, quotes, `]]>`, a tab, a space and non-ASCII characters are deleted over HTTP. The server checks the body's `Content-MD5`. Base64 matches RFC 4648's vectors, and the MD5 of an empty body is `1B2M2Y8AsgTpgAmY7PhCfg==`. The answer scanner finds an `<Error>` split across any chunking, and a long list of `<Deleted>` entries is success.
- AC-5: on the filesystem store `delete_many` checks every key first, then sends one free delete a key.
- AC-6: with `RUSTFS_BIN` set, the RustFS scenario also batch-deletes a stored key and a missing one.
- AC-7: `Rounds` logs a 2,001-key batch delete as 3 sequential rounds, one a batch, each named by its first key.
- AC-8: the node log's `delete_range` of 1,200 slots costs 2 `Delete` requests and no LIST.

## Test plan

- AC-1 to AC-3: `tests/emulator/deletes.rs` and `tests/s3/deletes.rs`. Each was seen failing against the trait's one-key-a-call default: 2,502 `Delete` requests instead of 3.
- AC-4: `tests/s3/deletes.rs`, seen failing against a client that sent the wrong digest and against one that did not escape keys. Unit tests in `s3/delete.rs` cover the scanner, base64 and the body.
- AC-5: `tests/fs/writes.rs`. AC-6: `tests/rustfs.rs`. AC-7: `tests/rounds.rs`. AC-8: `deletes_go_by_name` in `crates/writer/tests/node_log_store.rs`, seen failing at 1,200 requests.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | Depth |
|---|---|---|---|---|
| `delete_many` of n keys | 0 | 0 | 0 | ceil(n / 1000), each one free `Delete` per attempt |

S3 lists DELETE requests as free, and a `DeleteObjects` request is a delete. The meter records each batch as one `S3Op::Delete`, which is `Free`. The node log's trim drops from n round trips to ceil(n / 1000).

## Switch

None. Each batch replaces the same number of free DELETEs with one free request, and the outcome is the same: no request is billed and no behaviour differs. The only cost is memory: the body is built whole, about 6 MiB at worst for 1,000 keys of 1,024 escaped bytes, and tens of KiB for node-log keys.

## Risks

- A store that does not support `DeleteObjects`, or checks only a newer checksum header, would refuse every batch. RustFS is covered by AC-6 when its binary is available. The emulator checks `Content-MD5`, the header S3 has always required.
- A store that ignores quiet mode answers with every deleted key. The scanner streams through that answer without holding it.
