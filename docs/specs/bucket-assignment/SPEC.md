# Bucket assignment: which bucket a new small index joins

## Serves

R11. Ported from the shell's `BucketAssigner` (`STRIPES`, `Limits`, `stripe`, `bucketId`, `join`, `leave`) and the open-bucket half of `BucketStore` (`readOpen`, `swapOpen`, `OPEN_ROOT`) at huyz0/OpenSearch `429b4cbc`, on [bucket-commits](../bucket-commits/SPEC.md) and under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). New indices are spread over 16 stripes by their group (a tenant, say), so a tenant's indices made around the same time share buckets and creates in different stripes never contend. Each stripe has one open bucket, named by a generation in the stripe's register `bucket-open/<stripe>`; a full bucket moves the stripe on. A bucket is full by data size, with a member count and a stored manifest size as ceilings for buckets of indices too small to fill it by bytes.

## Change

- `oss_format`: `BUCKET_OPEN_PREFIX` (`bucket-open/`), `bucket_open_key(base, stripe)`, `encode_open(bucket_generation)` and `parse_open(bytes)`: the stripe register's value is the bucket generation in decimal ASCII.
- `oss_writer::bucket`: `BucketStore::read_open(stripe)` and `swap_open(stripe, bucket_generation, expected_generation)`; `BucketRead` also carries the register value's stored size; `BucketAssigner::new(store, limits)` with `Limits { target_bytes, max_members, max_manifest_bytes }`, `stripe(group)`, `bucket_id(stripe, generation)`, `join(group, index_name, index_uuid)` giving the bucket the index joined, and `leave(bucket, index_uuid)`.
- A stripe is the low four bits of the 64-bit routing hash of the group (murmur3 x64 128-bit, seed 0, `h1`), which is `floorMod(h1, 16)`; a bucket id is `s<stripe, two digits>-<generation>`.
- `join` is done before an index's descriptor names its bucket: the manifest is the truth, so an index the manifest does not list is never served as a member. It adds the index as a member with no commit (version 0) by one compare-and-swap of the bucket manifest, creating the manifest if the bucket has none. A full bucket moves the stripe's open register on to the next generation, whoever gets there first, and the join goes on in the new bucket. Up to 16 attempts, then `BucketError::Busy`.
- `leave` removes the member again, best effort, for a create that failed after joining.

Stricter than the Java: `Limits::new` clamps `max_members` to the manifest's 1,024-member ceiling (the Java's setting can be higher; [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md)), and refuses nothing else. Not here: bucket heads and ownership, bucket settings and their defaults, the descriptor's bucket field, node wiring. Nothing in a node calls this yet.

## Acceptance criteria

- AC-1: `stripe("hello")` is 2 (the Java's recorded hash of `hello` is `cbd8a7b341bd9b02`), a stripe is under 16 for every group, and `bucket_id(2, 0)` is `s02-0`, `bucket_id(11, 37)` is `s11-37`.
- AC-2: the first `join` in a deployment is two GETs (the open register and the bucket's manifest, both absent) and one create of the manifest; the member is in it with version 0 and no commit; the open register is not written.
- AC-3: `join` of an index already in the bucket returns its bucket with no write.
- AC-4: indices of the same group join the same bucket one after the other, each as one GET of the open register, one of the manifest and one conditional PUT; the manifest then holds every member.
- AC-5: a bucket that is full by member count, by data bytes (the sum of its members' recorded file lengths) or by stored manifest size takes no new member: the stripe's open register moves to the next generation (created with `1` when it was absent), and the index joins `s<stripe>-<generation+1>`; a bucket one under each limit still takes one.
- AC-6: a manifest swap lost to another writer is retried against a fresh read, and the join lands; sixteen lost swaps are `BucketError::Busy`.
- AC-7: `leave` removes the member from its bucket, leaves the others, and does nothing for a bucket with no register or an index that is not a member.
- AC-8: `Limits::new` clamps each bound to at least 1 and `max_members` to 1,024.
- AC-9: the open register's value is the generation in decimal ASCII; `parse_open` accepts it with surrounding whitespace and refuses an empty value, a sign, a non-digit and a number past `i64`; `bucket_open_key` is `<base>bucket-open/<stripe>` and refuses a base that is not empty and does not end in `/`.

## Tests

`crates/writer/tests/it/bucket_assign.rs` over the in-process emulator with its request meter; `crates/format/tests/bucket_open.rs`.

## S3 request budget

Per `join` that adds a member: one GET of the stripe's open register, one of the bucket's manifest, one conditional PUT of the manifest (a create when the bucket is new); a full bucket adds one conditional PUT of the open register; a lost swap adds the two GETs and a PUT again. 0 LIST. Index creation is rare against index use, and an idle index makes no request. `leave` is one GET and one PUT.

## Risks

- Every join of a stripe's open bucket rewrites that bucket's whole manifest (up to 1,024 members), so creation cost grows with the bucket; the member ceiling and the 2 MiB stored-size ceiling bound it.
- The stripe hash is checked against the one recorded Java hash and the shared routing hash tests, not against a Java-built bucket id; the R7 differential suite is the first independent check.
