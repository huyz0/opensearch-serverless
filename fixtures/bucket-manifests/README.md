# Bucket manifests written by the Java shell

Each file is the value of a bucket manifest register (the bytes after the generation) for the same logical bucket `s01-2` at term 9, written by `BucketManifest.toBytes` and `toBytes(codec)` from `huyz0/OpenSearch` at `feature/serverlessshell` (`serverless/shell/.../bucket/BucketManifest.java`, `CompactManifestFormat.java`, `ManifestCodec.java`), compiled and run with no change to the sources:

- `bucket.json`: the JSON form. `bucket-none.bin`, `bucket-deflate.bin`, `bucket-zstd.bin`, `bucket-lz4_fast.bin`: the compact form in each codec.
- `one-member.json`: a bucket of only `uuid-a`.
- `length-disagrees.json`: as `bucket.json` but a file's recorded length (41) differs from its location's range (40). The Java reads it; the Rust refuses it (see the spec).

Members: `uuid-a` (`logs-a`, version 5, a commit of six files with node-pack, segment-blob and shard-path locations, lengths, a pruning digest, stage sequence 12), `uuid-b` (`logs-b`, no commit), `uuid-c` (`logs-c`, version 1, a one-file commit with no writer).

Regenerate: compile the shell's `bucket` and `store` sources against `opensearch-3.3.0`, `opensearch-core`, `opensearch-x-content`, `lucene-core`, `zstd-jni`, `joda-time` and the Jackson jars, and run the generator that builds the manifest above (the sample in `BucketCompactManifestTests`).

## placement.tsv

Written by the Java `BucketPlacement` (`rank`, `keeps`, `observe`) over a fixed fleet, and `BucketAssigner`'s `stripe` and `bucketId`, which are copied line for line into the generator because the class itself needs the shell's blob register API. Lines: `stripe <group> <n>`, `bucketid <stripe> <generation> <id>`, `rank <bucket> <nodes best first>`, `keeps <bucket> <holder> <bool>`, `holddown <millis> <bucket> <nodes>`. The fleet is at time 1,000,000: node-0 (held 10), node-1 (12), node-2 (50), node-3 (a search node), node-4 (no load report), node-5 (9), node-6 (revoked), node-7 (expired), node-10 (10), node-9 (12), each with cap 100; the hold-down lines start from node-0 to node-2 seen at time 0 and add node-5 at the stated time with a 120,000 ms hold-down.
