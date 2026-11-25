# Registers: compare-and-swap blobs on every store

## Serves

The plan's storage-backend milestone (the filesystem-store task's generation-checked swap), and what metadata writes need next: descriptors, the register map and names are registers in the Java shell. Invariant I-1 (every store call metered) and the fencing rule (CAS on a blob, no locks or leases for correctness, except the filesystem's own, as the Java's).

## Change

Decision record: [ADR 0007](../../adr/0007-registers.md) (Accepted).


- `BlobStore` gains the four register operations of the Java's `ConditionalBlobContainer` (huyz0/OpenSearch `server/src/main/java/org/opensearch/common/blobstore/ConditionalBlobContainer.java@3920dc36`): `read_register`, `swap_register` (compare-and-swap on a generation), `create_register` (swap from absent in one request) and `delete_register` (delete only if unchanged).
- One wire format on every backend, the Java's: an 8-byte big-endian generation, then the value. A register starts at a random generation in `[2^32, 2^62)` with bit 32 set (`BlobRegister.initialGeneration`), drawn from the OS's secure source, and each swap adds one. A refused swap reports the generation it saw, or none when the store could not tell it atomically.
- Emulator and S3 (`S3BlobContainer`'s register methods, `plugins/repository-s3/src/main/java/org/opensearch/repositories/s3/S3BlobContainer.java@3920dc36`): every object gets an ETag; PUT takes `If-Match` as well as `If-None-Match: *`, DELETE takes `If-Match`, and GET, HEAD and PUT return the ETag. A swap reads the generation and ETag with one 8-byte ranged GET and writes with one conditional PUT; when the store remembers the ETag of the very generation expected (the last one it read or wrote, at most 10,000 registers remembered, oldest forgotten first), it skips the GET. A conditional PUT carries the writer token, so a retry refused after a lost response is told apart from another writer's object by one HEAD, as `create_if_absent` already does.
- Filesystem (`FsBlobRegisters`, `server/src/main/java/org/opensearch/common/blobstore/fs/FsBlobRegisters.java@3920dc36`): the same protocol as the Java, so Rust and Java processes on one host exclude each other: an in-process mutex stripe and an exclusive whole-file lock on `<real parent>/.opensearch-register-locks#/<stripe>`, the stripe being `floorMod(String.hashCode(path), 64)` of the register's canonical path. The lock is the one Java's `FileChannel.lock` takes: a POSIX `fcntl` record lock on Unix (Rust's `File::lock` is `flock`, which does not exclude it), `LockFileEx` on Windows. The register file is replaced by an atomic rename of a synced staged file in the lock directory; a reader takes no lock.
- Not in this change: callers (metadata writes are the next task), the S3 `x-amz-meta` writer token on plain puts, batch deletes.

## Acceptance criteria

- AC-1: on the emulator, S3 over loopback and fs, a register read after create returns the value and a generation in `[2^32, 2^62)` with bit 32 set; a swap from it applies with generation + 1; a swap from a stale generation is a conflict reporting the current one (none when the swap was from absent on an object store); a create of an existing register is a conflict; a delete with the current generation removes it and a read is then `None`; a delete with another generation leaves it.
- AC-2: of 8 threads swapping one register from the same generation, exactly one applies, and the stored value is that thread's, on each of the three backends; the same for 8 racing creates.
- AC-3: requests, emulator and S3, counted on the meter: read 1 `Get`; create 1 `Put`; swap against the generation last read or written 1 `Put`; swap from the absent generation 1 `Put` (the conditional create, refused with a conflict when the register exists); swap otherwise 1 `GetRange` and 1 `Put`; delete against a remembered generation 1 `Delete`, otherwise 1 `GetRange` and 1 `Delete`; a swap or delete whose read finds another generation, or none, stops after the `GetRange`. Fs: read 1 `Get`, create 1 `Put`, swap 1 `GetRange` and 1 `Put`, delete 1 `GetRange` and 1 `Delete` (what S3 would do without the memory).
- AC-4: a remembered ETag that went stale (another writer replaced the register, even at the same generation) costs a refused PUT and a conflict, never a wrong write; the next swap reads afresh. A conditional PUT that S3 answers 404 (the object is gone) is a conflict too, as the Java's `isConditionFailure`. Fs keeps no memory: it compares generations under the lock, as the Java.
- AC-5: S3: a conditional PUT whose response is lost after it committed, then refused on retry, answers `Applied` after one HEAD finds its writer token; one refused because another writer's object is there answers a conflict.
- AC-6: the register `fixtures/shell-blobs/indices/fixture`, written by the Java shell's filesystem store, reads on `FsStore` with the generation in its first 8 bytes and the rest as its value.
- AC-7: fs: while another process holds the POSIX record lock on a register's stripe file, a swap of that register waits until it is released (a child process of the test holds it); the stripe of a path equals Java's `floorMod(String.hashCode(path), 64)` for ASCII and non-ASCII paths.
- AC-8: a register value over 16 MiB, a read whose register does not fit the buffer, and an object shorter than 8 bytes are refused with `InvalidArgument` or `Protocol`, never truncated. A register body streams from the caller's value; no copy of the whole is made.
- AC-9: every request-path allocation states its bound (`scripts/check-alloc.py`).

## Test plan

- AC-1 to AC-4, AC-8: `crates/blob/tests/registers/`, one suite run over the three backends, with exact meter counts per backend; seen failing against planted defects: a swap that skips the conditional header, a remembered ETag used for another generation, a create that reads first.
- AC-5: the S3 front end's `lose_response_next(Put, 1)`.
- AC-6: the fixture.
- AC-7: the test binary re-runs itself as a child that takes the lock with the same syscall Java does and holds it; the stripe hash against values computed by Java's `String.hashCode` and checked once by hand against a JVM.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | Depth |
|---|---|---|---|---|
| `read_register` | 0 | 1 | 0 | 1 |
| `create_register` | 1 | 1 HEAD only when a retry is refused | 0 | 1 |
| `swap_register`, generation remembered | 1 | as above | 0 | 1 |
| `swap_register`, otherwise | 1 | 1 ranged GET (8 bytes) | 0 | 2 |
| `delete_register` | 0 | 1 ranged GET unless remembered | 0 | 1 or 2 |

The same as the Java's, which reads the whole register where this reads 8 bytes.

## Switch

None: new operations nothing calls yet.

## Risks

- A conditional PUT whose response is lost, then overtaken by another writer before the retry, answers `Conflict(None)` although it applied: the retry's HEAD finds the other writer's token. Fail-safe, as the Java; callers read before deciding again.
- The S3 version memory (up to about 11 MiB per store) is not reserved from the node budget, and registers bypass `PooledStore`; the first caller wires both.

- An empty object reads as no register on every backend (the Java's filesystem rule), but only fs can swap from it: on S3 there is no generation to compare and an 8-byte ranged GET of it is refused, so the swap fails with `Protocol`. Nothing writes empty registers.

- A conditional DELETE retried after a lost success finds nothing and answers `false`; the Java's SDK retries the same way. Callers treat `false` as "read again".
- An S3-compatible store that ignores `If-Match` on DELETE deletes regardless; the Java probes for it (`ConditionalDeleteProbe`) before relying on it, and so must the first caller here.
- The filesystem lock is a POSIX record lock: closing any descriptor of a stripe file in this process releases it, so only this module opens stripe files, one at a time per stripe under its mutex, as the Java does.
