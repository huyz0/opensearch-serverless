# Bucket settings: the node settings that turn buckets on and size them

## Serves

R11. Ported from the shell's `BucketSettings` (`ENABLED`, `MAX_MEMBERS`, `TARGET_BYTES`, `MAX_MANIFEST_BYTES`, `MANIFEST_CODEC`, `limits`, `manifestCodec`, `compressOnlyIfWorthIt`) at huyz0/OpenSearch `429b4cbc`, under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). Buckets are a switch: off until a measurement shows the savings with no cost, so `serverless.buckets.enabled` defaults to `false` and nothing reads the other four while it is off. The sizing settings feed [bucket assignment](../bucket-assignment/SPEC.md) and the manifest write form of [bucket commits](../bucket-commits/SPEC.md).

## Change

- `oss_writer::bucket::settings`, listed in the node's registry under the `writer` feature:
  - `serverless.buckets.enabled`, bool, default `false`.
  - `serverless.buckets.max_members`, int, default 1,024, at least 1.
  - `serverless.buckets.target_bytes`, long, default 1 GiB, at least 1.
  - `serverless.buckets.max_manifest_bytes`, int, default 2 MiB, at least 1.
  - `serverless.buckets.manifest_codec`, one of `auto` (default), `json`, `none`, `deflate`, `zstd`.
- `enabled(source)`, `limits(source) -> Limits` and `manifest_write(source) -> ManifestWrite`: `auto` is the compact form compressed with zstd only when that pays, `json` is JSON, and a codec name is the compact form always compressed with it.

Stricter than the Java: a size at or under zero is refused instead of clamped to 1, a codec name is lower case only, and `lz4` is refused because it is read and never written. `max_members` above 1,024 is still accepted and clamped by `Limits::new` ([ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md)). Not here: the index and cluster policies (`index.serverless.bucket`, `serverless.bucket`) and the group setting, which come with bucket placement; node wiring. Nothing in a node reads these yet.

## Acceptance criteria

- AC-1: with nothing set, buckets are off, the limits are 1 GiB, 1,024 members and 2 MiB, and manifests are written `auto` with zstd.
- AC-2: each setting reads its value from a source; `max_members` above 1,024 gives limits of 1,024.
- AC-3: a zero or negative size, and a value that is no number, is refused with the setting's key.
- AC-4: `manifest_write` is `Json` for `json`, `Compact(None | Deflate | Zstd)` for `none`, `deflate`, `zstd`, `Auto(Zstd)` for `auto`; any other name, `lz4` and `ZSTD` included, is refused.
- AC-5: the five settings are node scope, pass the registry's rules with the node's other settings, and are in the node's registry in a writer build.

## Tests

`crates/writer/tests/it/bucket_settings.rs`; the registry check is `the_nodes_settings_form_a_valid_registry` in `crates/node`.

## S3 request budget

None: reading a setting makes no request.

## Risks

- The switch has no reader in a node yet, so turning it on does nothing until the publisher and node wiring land.
